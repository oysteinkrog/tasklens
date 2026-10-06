//! The `Connections` worker (ADR-004, ADR-008, ADR-016 rows "Per-process network", "Socket to
//! pid map", "Connections"; SYS-028 to SYS-030, PROC-032).
//!
//! Every 2 s while the Connections tab or a process network column is shown: one sock_diag dump,
//! one rebuild of the socket inode to pid map, the connection records and the per-process TCP
//! attribution, all in one blocking job. UDP is dumped only while the Connections tab is shown.
//!
//! Address strings are formatted once per socket cookie by `ConnectionTable`. Addresses and
//! ports are never logged above debug level.

use std::cell::Cell;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::sync::Arc;
use std::time::Duration;

use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::{Cx, Outcome};
use rustc_hash::FxHashMap;
use tl_collect::counters::{Clock, resumed_between};
use tl_collect::net::sock_diag::{self, SockDiag, SockRecord};
use tl_collect::net::{
    ConnRecord as RawRecord, ConnectionTable, IfNames, NetRate, Owner, SocketMap,
};
use tl_collect::{CollectError, SysRoot};
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::demand::Demand;
use tl_model::health::{SourceId, SourceState};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ConnRecord, ConnState, TcpInfoView};
use tl_model::time::BootId;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::source::{self, Source, WorkerIo};
use crate::supervise::WorkerFactory;
use crate::update::{ConnectionsUpdate, SourceUpdate, WorkerError};
use crate::workers::WorkerSpec;

pub const PERIOD: Duration = Duration::from_secs(2);

/// Process columns that need the per-process attribution.
const NET_COLUMNS: [ColumnId; 3] = [
    ColumnId::Network,
    ColumnId::NetSendRate,
    ColumnId::NetRecvRate,
];

/// UDP socket state 7 (CLOSE) means unconnected, 1 (ESTABLISHED) connected.
const UDP_CONNECTED: u8 = 1;
const UDP_UNCONNECTED: u8 = 7;

/// The ADR-008 row "Connections and socket attribution": on for the tab or a network column.
pub fn connections_period(d: &Demand, _s: &EngineSettings) -> Option<Duration> {
    (d.connections || wants_attribution(d)).then_some(PERIOD)
}

fn wants_attribution(d: &Demand) -> bool {
    d.process_table && NET_COLUMNS.iter().any(|&c| d.process_columns.contains(c))
}

/// The kernel reads of one sample. Tests replace it with fixtures.
pub trait SocketReader: Send + 'static {
    /// TCP over IPv4 and IPv6 with `INET_DIAG_INFO`. Clears `out` first.
    fn dump_tcp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError>;
    /// UDP over IPv4 and IPv6. Appends to `out`.
    fn dump_udp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError>;
    /// Rebuilds `map` from `/proc/<pid>/fd`.
    fn rebuild_map(&mut self, map: &mut SocketMap) -> Result<(), CollectError>;
    /// Interface names for link-local scopes and the Interface column.
    fn if_names(&mut self) -> IfNames;
}

/// The real system: one `NETLINK_SOCK_DIAG` socket, opened on first use and again after an
/// error.
pub struct HostReader {
    root: SysRoot,
    boot: BootId,
    diag: Option<SockDiag>,
}

impl HostReader {
    pub fn new(root: SysRoot, boot: BootId) -> Self {
        Self {
            root,
            boot,
            diag: None,
        }
    }

    fn with_diag(
        &mut self,
        f: impl FnOnce(&mut SockDiag) -> Result<(), CollectError>,
    ) -> Result<(), CollectError> {
        let diag = match &mut self.diag {
            Some(diag) => diag,
            empty => empty.insert(SockDiag::open()?),
        };
        let result = f(diag);
        if result.is_err() {
            self.diag = None;
        }
        result
    }
}

impl SocketReader for HostReader {
    fn dump_tcp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
        self.with_diag(|diag| diag.dump_tcp(out))
    }

    fn dump_udp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
        self.with_diag(|diag| {
            diag.dump(sock_diag::Family::V4, sock_diag::Proto::Udp, out)?;
            diag.dump(sock_diag::Family::V6, sock_diag::Proto::Udp, out)
        })
    }

    fn rebuild_map(&mut self, map: &mut SocketMap) -> Result<(), CollectError> {
        map.rebuild(&self.root, self.boot)
    }

    fn if_names(&mut self) -> IfNames {
        IfNames::read(&self.root).unwrap_or_default()
    }
}

pub struct ConnectionsSource<R: SocketReader> {
    reader: R,
    /// Set by `period()` from the latest demand: the Connections tab is shown, so UDP is dumped
    /// and records are built.
    tab_shown: Cell<bool>,
    socks: Vec<SockRecord>,
    map: SocketMap,
    /// Keeps the address strings per cookie between samples.
    table: ConnectionTable,
    attribution: tl_collect::net::TcpAttribution,
    /// The clocks of the previous sample, for the ADR-008 suspend check.
    last_clock: Option<Clock>,
    /// The last sample failed, so the next good one reports `Running` again.
    failed: bool,
    pending: Option<SourceUpdate>,
}

impl<R: SocketReader> ConnectionsSource<R> {
    pub fn new(reader: R) -> Self {
        Self {
            reader,
            tab_shown: Cell::new(false),
            socks: Vec::new(),
            map: SocketMap::new(),
            table: ConnectionTable::new(),
            attribution: tl_collect::net::TcpAttribution::new(),
            last_clock: None,
            failed: false,
            pending: None,
        }
    }

    /// `sample()` is `self.sample_at(Clock::now())`; tests call this with an injected clock.
    pub fn sample_at(mut self, clock: Clock) -> (Self, SourceUpdate) {
        let tab_shown = self.tab_shown.get();
        if let Err(e) = self.read(tab_shown) {
            return self.failure(clock, &e);
        }
        tracing::debug!(
            elapsed_ms = self.map.elapsed_ns as f64 / 1e6,
            inodes = self.map.owners.len(),
            restricted = self.map.restricted.len(),
            "socket map rebuilt"
        );

        // A boottime jump over the monotonic one means the machine slept: no rates this time.
        if self
            .last_clock
            .is_some_and(|prev| resumed_between(prev, clock))
        {
            self.attribution.reset();
        }
        self.last_clock = Some(clock);

        // The TCP records come first in `socks`; UDP records have no tcp_info and are skipped.
        self.attribution
            .update(&self.socks, &self.map, clock.boot_ns);
        let attribution = self.attribution_map();

        let records = if tab_shown {
            let ifnames = self.reader.if_names();
            self.table.update(&self.socks, &self.map, &ifnames);
            self.table.records.iter().map(to_model).collect()
        } else {
            Vec::new()
        };

        if std::mem::take(&mut self.failed) {
            self.pending = Some(health(SourceState::Running, None, clock));
        }
        let update = ConnectionsUpdate {
            t_ns: clock.boot_ns,
            records,
            validity: Validity::Valid,
            attribution,
        };
        (self, SourceUpdate::Connections(update))
    }

    fn read(&mut self, tab_shown: bool) -> Result<(), CollectError> {
        self.reader.dump_tcp(&mut self.socks)?;
        if tab_shown {
            self.reader.dump_udp(&mut self.socks)?;
        }
        self.reader.rebuild_map(&mut self.map)
    }

    /// The attribution's per-process rates. On a warming-up pass every owner of a TCP socket
    /// gets a `WarmingUp` entry, so the column shows no value rather than 0 (R-162).
    fn attribution_map(&self) -> FxHashMap<ProcessKey, NetRate> {
        if self.attribution.validity != Validity::WarmingUp {
            return self.attribution.per_process.clone();
        }
        let mut out = FxHashMap::default();
        for sock in &self.socks {
            if sock.proto != sock_diag::Proto::Tcp || sock.tcp_info.is_none() || sock.inode == 0 {
                continue;
            }
            let owners = self.map.owners_of(sock.inode);
            let shared = owners.len() >= 2;
            for &key in owners {
                let entry = out.entry(key).or_insert(NetRate {
                    rx_bps: 0.0,
                    tx_bps: 0.0,
                    shared: false,
                    validity: Validity::WarmingUp,
                });
                entry.shared |= shared;
            }
        }
        out
    }

    /// A failed dump or map rebuild: `Failed` with the error in `last_error`, and no rates
    /// until two good samples in a row.
    fn failure(mut self, clock: Clock, e: &CollectError) -> (Self, SourceUpdate) {
        self.failed = true;
        self.attribution.reset();
        self.last_clock = None;
        self.pending = Some(health(
            SourceState::Failed,
            Some(format!("socket dump: {e}").into()),
            clock,
        ));
        let update = ConnectionsUpdate {
            t_ns: clock.boot_ns,
            records: Vec::new(),
            validity: Validity::Failed,
            attribution: FxHashMap::default(),
        };
        (self, SourceUpdate::Connections(update))
    }
}

impl<R: SocketReader> Source for ConnectionsSource<R> {
    fn id(&self) -> SourceId {
        SourceId::Connections
    }

    fn period(&self, d: &Demand, s: &EngineSettings) -> Option<Duration> {
        self.tab_shown.set(d.connections);
        connections_period(d, s)
    }

    fn sample(self) -> (Self, SourceUpdate) {
        self.sample_at(Clock::now())
    }

    fn reset_baselines(&mut self) {
        self.attribution.reset();
        self.last_clock = None;
    }

    fn take_extra(&mut self) -> Option<SourceUpdate> {
        self.pending.take()
    }
}

fn health(state: SourceState, last_error: Option<Arc<str>>, clock: Clock) -> SourceUpdate {
    SourceUpdate::Health {
        id: SourceId::Connections,
        state,
        last_error,
        t_ns: clock.boot_ns,
    }
}

/// The tl-model record the Connections table shows (UI-SYS-062).
fn to_model(r: &RawRecord) -> ConnRecord {
    let s = &r.sock;
    let (owners, owner_validity) = match &r.owner {
        Owner::Kernel => (Default::default(), Validity::Valid),
        Owner::Processes(keys) if keys.is_empty() => (keys.clone(), Validity::Missing),
        Owner::Processes(keys) => (keys.clone(), Validity::Valid),
        Owner::Restricted { .. } => (Default::default(), Validity::Restricted),
    };
    let (proto, state) = match s.proto {
        sock_diag::Proto::Tcp => (Proto::Tcp, ConnState::from_tcp_state(s.state)),
        sock_diag::Proto::Udp => (
            Proto::Udp,
            match s.state {
                UDP_CONNECTED => ConnState::Connected,
                UDP_UNCONNECTED => ConnState::Unconnected,
                _ => ConnState::Unknown,
            },
        ),
    };
    let family = match s.family {
        sock_diag::Family::V4 => Family::Inet,
        sock_diag::Family::V6 => Family::Inet6,
    };
    ConnRecord {
        cookie: r.cookie,
        proto,
        family,
        four_tuple: FourTuple {
            local: socket_addr(s.src, s.sport, s.ifindex),
            remote: socket_addr(s.dst, s.dport, s.ifindex),
        },
        local: Arc::clone(&r.local),
        foreign: Arc::clone(&r.foreign),
        state,
        owners,
        owner_validity,
        uid: s.uid,
        inode: s.inode,
        ifindex: s.ifindex,
        recv_q: s.rqueue,
        send_q: s.wqueue,
        // Unit resolution for a socket owner is not part of this worker.
        unit: None,
        tcp: s.tcp_info.map(|t| TcpInfoView {
            rtt_us: t.rtt_us,
            rttvar_us: t.rttvar_us,
            snd_cwnd: t.snd_cwnd,
            bytes_acked: t.bytes_acked,
            bytes_received: t.bytes_received,
            retransmits: t.total_retrans,
            last_data_recv_ms: t.last_data_recv_ms,
        }),
        remote_host: None,
    }
}

/// A link-local IPv6 address keeps its interface as the scope id, so SockDestroy can name it
/// (R-270).
fn socket_addr(ip: IpAddr, port: u16, ifindex: u32) -> SocketAddr {
    match ip {
        IpAddr::V6(v6) if v6.is_unicast_link_local() => {
            SocketAddr::V6(SocketAddrV6::new(v6, port, 0, ifindex))
        }
        ip => SocketAddr::new(ip, port),
    }
}

/// In the full engine only.
pub fn spec() -> WorkerSpec {
    WorkerSpec {
        id: SourceId::Connections,
        name: "connections",
        modes: &[EngineMode::Full],
        factory,
    }
}

/// A new `ConnectionsSource` per generation. The boot id is read on the blocking pool.
pub fn factory(cfg: Arc<EngineConfig>, io: WorkerIo) -> WorkerFactory {
    Arc::new(move |cx: Cx, _generation: ManagedGeneration| {
        let io = io.clone();
        let root = cfg.sys_root.clone();
        async move {
            let mut job = match cx.spawn_blocking(move |_| read_boot_id(&root).map(|b| (root, b))) {
                Ok(job) => job,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let (root, boot) = match job.join(&cx).await {
                Ok(Ok(found)) => found,
                Ok(Err(e)) => return Outcome::Err(WorkerError::Collect(e)),
                Err(JoinError::Cancelled(reason)) => {
                    if cx.checkpoint().is_err() {
                        return Outcome::Ok(());
                    }
                    return Outcome::Cancelled(reason);
                }
                Err(JoinError::Panicked(payload)) => return Outcome::Panicked(payload),
                Err(JoinError::PolledAfterCompletion) => {
                    return Outcome::Err(WorkerError::Other("job polled after completion".into()));
                }
            };
            let src = ConnectionsSource::new(HostReader::new(root, boot));
            source::run(cx, io, src).await
        }
    })
}

fn read_boot_id(root: &SysRoot) -> Result<BootId, CollectError> {
    let text = std::fs::read_to_string(root.proc.join("sys/kernel/random/boot_id"))?;
    BootId::from_uuid_text(&text).ok_or(CollectError::Parse {
        file: "boot_id",
        what: "format",
    })
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU32, Ordering};

    use asupersync::channel::{mpsc, watch};
    use asupersync::types::Budget;
    use asupersync::{LabConfig, LabRuntime};
    use smallvec::SmallVec;
    use tl_collect::net::sock_diag::{TcpInfoLite, parse_dump};
    use tl_model::demand::ColumnSet;

    use super::*;
    use crate::source::WorkerControl;
    use crate::update::EngineMsg;

    const SEC: u64 = 1_000_000_000;
    const BOOT: BootId = BootId(7);

    fn fixture_root() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tl-collect/tests/fixtures/sockets-basic")
    }

    /// Every record of one fixture dump file.
    fn fixture_dump(
        name: &str,
        family: sock_diag::Family,
        proto: sock_diag::Proto,
    ) -> Vec<SockRecord> {
        let bytes = std::fs::read(fixture_root().join("dump").join(name)).expect("fixture dump");
        let mut out = Vec::new();
        assert!(parse_dump(&bytes, family, proto, &mut out).expect("parses"));
        out
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BOOT,
            pid,
        }
    }

    fn tcp(cookie: u64, inode: u64, acked: u64, received: u64) -> SockRecord {
        SockRecord {
            family: sock_diag::Family::V4,
            proto: sock_diag::Proto::Tcp,
            state: 1,
            src: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 2)),
            dst: IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9)),
            sport: 40000,
            dport: 443,
            ifindex: 0,
            cookie,
            rqueue: 0,
            wqueue: 0,
            uid: 1000,
            inode,
            tcp_info: Some(TcpInfoLite {
                bytes_acked: acked,
                bytes_received: received,
                ..TcpInfoLite::default()
            }),
        }
    }

    /// Scripted dumps. `tcp` is the next TCP dump; `udp_calls` counts UDP dumps.
    #[derive(Default)]
    struct Fake {
        tcp: Vec<SockRecord>,
        udp: Vec<SockRecord>,
        owners: Vec<(u64, Vec<ProcessKey>)>,
        /// `Some` builds the map from this fixture root instead of `owners`.
        root: Option<SysRoot>,
        udp_calls: Arc<AtomicU32>,
        fail: bool,
    }

    impl SocketReader for Fake {
        fn dump_tcp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
            if self.fail {
                return Err(CollectError::SockDiagErrno(1));
            }
            out.clear();
            out.extend(self.tcp.iter().cloned());
            Ok(())
        }

        fn dump_udp(&mut self, out: &mut Vec<SockRecord>) -> Result<(), CollectError> {
            self.udp_calls.fetch_add(1, Ordering::SeqCst);
            out.extend(self.udp.iter().cloned());
            Ok(())
        }

        fn rebuild_map(&mut self, map: &mut SocketMap) -> Result<(), CollectError> {
            if let Some(root) = &self.root {
                return map.rebuild(root, BOOT);
            }
            map.owners.clear();
            for (inode, keys) in &self.owners {
                map.owners.insert(*inode, keys.iter().copied().collect());
            }
            Ok(())
        }

        fn if_names(&mut self) -> IfNames {
            IfNames::default()
        }
    }

    fn at(boot_s: u64, mono_s: u64) -> Clock {
        Clock {
            boot_ns: boot_s * SEC,
            mono_ns: mono_s * SEC,
        }
    }

    /// Runs `period` with `d` (as the loop does), then one sample.
    fn sample(
        src: ConnectionsSource<Fake>,
        d: &Demand,
        clock: Clock,
    ) -> (ConnectionsSource<Fake>, ConnectionsUpdate) {
        let _ = src.period(d, &EngineSettings::default());
        match src.sample_at(clock) {
            (src, SourceUpdate::Connections(update)) => (src, update),
            (_, other) => panic!("not a connections update: {other:?}"),
        }
    }

    fn tab() -> Demand {
        Demand {
            connections: true,
            ..Demand::default()
        }
    }

    fn net_column(c: ColumnId) -> Demand {
        let mut cols = ColumnSet::default();
        cols.insert(c);
        Demand {
            process_table: true,
            process_columns: cols,
            ..Demand::default()
        }
    }

    #[test]
    fn period_off_without_demand() {
        let s = EngineSettings::default();
        assert_eq!(connections_period(&Demand::default(), &s), None);
        let mut cols = ColumnSet::default();
        cols.insert(ColumnId::Cpu);
        let d = Demand {
            process_table: true,
            process_columns: cols,
            ..Demand::default()
        };
        assert_eq!(connections_period(&d, &s), None);
        // A network column without the process table needs nothing.
        let mut hidden = net_column(ColumnId::Network);
        hidden.process_table = false;
        assert_eq!(connections_period(&hidden, &s), None);
    }

    #[test]
    fn period_on_for_network_columns() {
        let s = EngineSettings::default();
        for c in NET_COLUMNS {
            assert_eq!(
                connections_period(&net_column(c), &s),
                Some(PERIOD),
                "{c:?}"
            );
        }
        assert_eq!(PERIOD, Duration::from_secs(2));
    }

    #[test]
    fn period_on_for_tab() {
        assert_eq!(
            connections_period(&tab(), &EngineSettings::default()),
            Some(PERIOD)
        );
    }

    #[test]
    fn no_udp_dump_when_tab_hidden() {
        let udp_calls = Arc::new(AtomicU32::new(0));
        let fake = Fake {
            tcp: fixture_dump("tcp4.bin", sock_diag::Family::V4, sock_diag::Proto::Tcp),
            udp: fixture_dump("udp4.bin", sock_diag::Family::V4, sock_diag::Proto::Udp),
            root: Some(SysRoot::at(&fixture_root())),
            udp_calls: Arc::clone(&udp_calls),
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let (src, update) = sample(src, &net_column(ColumnId::Network), at(10, 10));
        assert_eq!(udp_calls.load(Ordering::SeqCst), 0);
        assert!(update.records.is_empty());
        assert_eq!(update.validity, Validity::Valid);

        // With the tab shown, UDP is dumped and the records come sorted by cookie.
        let (_, update) = sample(src, &tab(), at(12, 12));
        assert_eq!(udp_calls.load(Ordering::SeqCst), 1);
        assert!(update.records.iter().any(|r| r.proto == Proto::Udp));
        assert!(update.records.windows(2).all(|w| w[0].cookie < w[1].cookie));
    }

    #[test]
    fn attribution_rate_from_elapsed_time() {
        let a = key(100);
        let fake = Fake {
            tcp: vec![tcp(7, 55, 1_000, 0)],
            owners: vec![(55, vec![a])],
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let d = net_column(ColumnId::NetSendRate);
        let (mut src, _) = sample(src, &d, at(10, 10));
        src.reader.tcp = vec![tcp(7, 55, 2_001_000, 500)];
        let (_, update) = sample(src, &d, at(12, 12));
        let r = update.attribution[&a];
        assert_eq!(r.tx_bps, 1_000_000.0);
        assert_eq!(r.rx_bps, 250.0);
        assert!(!r.shared);
        assert_eq!(r.validity, Validity::Valid);
    }

    #[test]
    fn attribution_first_sample_warming_up() {
        let a = key(100);
        let fake = Fake {
            tcp: vec![tcp(7, 55, 1_000, 0)],
            owners: vec![(55, vec![a])],
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let (_, update) = sample(src, &net_column(ColumnId::Network), at(10, 10));
        let r = update.attribution[&a];
        assert_eq!(r.validity, Validity::WarmingUp);
        assert_ne!(r.validity, Validity::Valid);
    }

    #[test]
    fn attribution_shared_socket() {
        // Inode 5001 is held by pids 100 and 200 in the fixture.
        let fake = Fake {
            tcp: vec![tcp(7, 5001, 0, 0)],
            root: Some(SysRoot::at(&fixture_root())),
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let d = net_column(ColumnId::Network);
        let (mut src, first) = sample(src, &d, at(10, 10));
        assert_eq!(first.attribution.len(), 2);
        assert!(first.attribution.values().all(|r| r.shared));
        src.reader.tcp = vec![tcp(7, 5001, 4_000, 2_000)];
        let (_, update) = sample(src, &d, at(12, 12));
        let pids: Vec<u32> = {
            let mut p: Vec<u32> = update.attribution.keys().map(|k| k.pid).collect();
            p.sort_unstable();
            p
        };
        assert_eq!(pids, [100, 200]);
        for r in update.attribution.values() {
            assert_eq!((r.tx_bps, r.rx_bps), (2_000.0, 1_000.0));
            assert!(r.shared);
            assert_eq!(r.validity, Validity::Valid);
        }
    }

    #[test]
    fn address_strings_cached_per_cookie() {
        let fake = Fake {
            tcp: vec![tcp(9, 55, 0, 0)],
            owners: vec![(55, vec![key(100)])],
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let (src, first) = sample(src, &tab(), at(10, 10));
        let (_, second) = sample(src, &tab(), at(12, 12));
        assert_eq!(&*first.records[0].local, "10.0.0.2:40000");
        assert!(Arc::ptr_eq(
            &first.records[0].local,
            &second.records[0].local
        ));
        assert!(Arc::ptr_eq(
            &first.records[0].foreign,
            &second.records[0].foreign
        ));
        assert_eq!(
            second.records[0].owners,
            SmallVec::<[ProcessKey; 1]>::from_slice(&[key(100)])
        );
        assert_eq!(second.records[0].owner_validity, Validity::Valid);
    }

    #[test]
    fn suspend_gap_drops_rates() {
        let a = key(100);
        let fake = Fake {
            tcp: vec![tcp(7, 55, 0, 0)],
            owners: vec![(55, vec![a])],
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let d = net_column(ColumnId::Network);
        let (mut src, _) = sample(src, &d, at(10, 10));
        src.reader.tcp = vec![tcp(7, 55, 1_000_000, 0)];
        // Boottime moved 5 s, monotonic 10 ms.
        let clock = Clock {
            boot_ns: 15 * SEC,
            mono_ns: 10 * SEC + 10_000_000,
        };
        let (_, update) = sample(src, &d, clock);
        assert!(
            update
                .attribution
                .values()
                .all(|r| r.validity == Validity::WarmingUp && r.tx_bps == 0.0),
            "{:?}",
            update.attribution
        );
    }

    #[test]
    fn dump_failure_is_failed_with_health() {
        let fake = Fake {
            fail: true,
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let (mut src, update) = sample(src, &tab(), at(10, 10));
        assert_eq!(update.validity, Validity::Failed);
        assert!(matches!(
            src.take_extra(),
            Some(SourceUpdate::Health {
                state: SourceState::Failed,
                last_error: Some(_),
                ..
            })
        ));
        src.reader.fail = false;
        let (mut src, update) = sample(src, &tab(), at(12, 12));
        assert_eq!(update.validity, Validity::Valid);
        assert!(matches!(
            src.take_extra(),
            Some(SourceUpdate::Health {
                state: SourceState::Running,
                ..
            })
        ));
    }

    #[test]
    fn demand_switches_rate() {
        let mut lab = LabRuntime::new(LabConfig::new(42).max_steps(10_000_000));
        let root = lab.state.create_root_region(Budget::INFINITE);
        let (updates, mut rx) = mpsc::channel::<EngineMsg>(256);
        let (demand_tx, demand) = watch::channel(Demand::default());
        let (_settings_tx, settings) = watch::channel(EngineSettings::default());
        let (_control_tx, control) = watch::channel(WorkerControl::default());
        let io = WorkerIo::new(
            updates,
            demand,
            settings,
            control,
            crate::dbus::SharedHandles::default(),
        );
        let src = ConnectionsSource::new(Fake::default());
        let (task, handle) = lab
            .state
            .create_task(root, Budget::INFINITE, async move {
                let cx = Cx::current().expect("task has a Cx");
                let _ = source::run(cx, io, src).await;
            })
            .expect("task is created");
        lab.scheduler.lock().schedule(task, 0);
        lab.run_until_idle();

        let advance = |lab: &mut LabRuntime, d: Duration| {
            lab.advance_time(u64::try_from(d.as_nanos()).expect("fits in u64"));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            lab.step_for_test();
            lab.run_until_idle();
        };
        let count = |rx: &mut mpsc::Receiver<EngineMsg>| {
            let mut n = 0;
            while let Ok(msg) = rx.try_recv() {
                if matches!(msg, EngineMsg::Source(SourceUpdate::Connections(_))) {
                    n += 1;
                }
            }
            n
        };

        // Off: the loop wakes every 500 ms and samples nothing.
        for _ in 0..120 {
            advance(&mut lab, Duration::from_millis(500));
        }
        assert_eq!(count(&mut rx), 0);

        demand_tx
            .send(net_column(ColumnId::NetRecvRate))
            .expect("watch has receivers");
        // Let the loop pick up the new period on its next idle wake.
        advance(&mut lab, Duration::from_millis(500));
        let _ = count(&mut rx);
        for _ in 0..30 {
            advance(&mut lab, Duration::from_secs(2));
        }
        assert_eq!(count(&mut rx), 30);

        handle.abort();
        lab.run_until_idle();
    }
}
