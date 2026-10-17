//! The `Connections` worker (ADR-004, ADR-008, ADR-016 rows "Per-process network", "Socket to
//! pid map", "Connections"; SYS-028 to SYS-030, PROC-032).
//!
//! Every 2 s while the Connections tab or a process network column is shown: one sock_diag dump,
//! one rebuild of the socket inode to pid map, the connection records and the per-process TCP
//! attribution, all in one blocking job. UDP is dumped only while the Connections tab is shown.
//!
//! Address strings are formatted once per socket cookie by `ConnectionTable`. Addresses and
//! ports are never logged above debug level.
//!
//! Reverse DNS for the Remote host column (SYS-035) runs in a second task of the same
//! generation: the source sends it the foreign addresses of the visible cells, and it sends the
//! names back, which the source caches by address. Looked-up names are never logged.
//!
//! While the helper is `Unlocked`, `SocketResolve` asks it for the owners of the sockets whose
//! process could not be read (`ResolveSocketInodes`, SYS-030, PROC-032), at most one request
//! per rebuild and outside it. The next rebuild merges the owners into the socket map, so the
//! Connections rows and the process network columns show them.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::future::Future;
use std::net::{IpAddr, SocketAddr, SocketAddrV6};
use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use asupersync::channel::{mpsc, watch};
use asupersync::runtime::JoinError;
use asupersync::supervision::ManagedGeneration;
use asupersync::time;
use asupersync::{Cx, Outcome};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_collect::counters::{Clock, resumed_between};
use tl_collect::net::sock_diag::{self, SockDiag, SockRecord};
use tl_collect::net::{
    ConnRecord as RawRecord, ConnectionTable, IfNames, NetRate, Owner, SocketMap,
};
use tl_collect::{CollectError, SysRoot};
use tl_model::actions::{Family, FourTuple, Proto};
use tl_model::demand::Demand;
use tl_model::health::{HelperState, SourceId, SourceState};
use tl_model::helper_proto::{HelperReply, HelperReplyBody, HelperRequest};
use tl_model::ids::{ColumnId, ProcessKey};
use tl_model::settings::EngineSettings;
use tl_model::snapshot::{ConnRecord, ConnState, RemoteHost, TcpInfoView};
use tl_model::time::BootId;
use tl_model::validity::Validity;

use crate::config::{EngineConfig, EngineMode};
use crate::helper::{HelperCallError, HelperHandle, HelperReadError};
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
/// A TCP socket in TIME-WAIT has no owner process: "No owner (kernel)".
const TCP_TIME_WAIT: u8 = 6;

/// Reverse DNS lookups that run at once (SYS-035). The blocking pool also has 4 threads
/// (ADR-004), so while 4 lookups run the 2 s sample waits for a thread.
const MAX_LOOKUPS: usize = 4;
/// A lookup that takes longer counts as a negative answer. The blocking call itself keeps
/// running until the resolver gives up.
const LOOKUP_TIMEOUT: Duration = Duration::from_secs(2);
/// How long a name is kept (SYS-035).
const POSITIVE_TTL: Duration = Duration::from_secs(600);
/// How long "no name" is kept (SYS-035).
const NEGATIVE_TTL: Duration = Duration::from_secs(60);
/// Cached addresses, and also the most addresses waiting for an answer.
const CACHE_MAX: usize = 4096;
/// An address with no answer after this long is asked for again, in case its answer was
/// dropped. The resolver ignores an address it already has.
const PENDING_STALE: Duration = Duration::from_secs(60);
/// Messages to the resolver task. Requests that do not fit are dropped and sent again on the
/// next sample.
const REQUEST_CAPACITY: usize = 256;
/// Answers to the source. An answer that does not fit is dropped; see `PENDING_STALE`.
const ANSWER_CAPACITY: usize = 256;

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
    /// Set by `period()` from the latest demand: the cookies of the visible Remote host cells.
    host_cookies: RefCell<Arc<[u64]>>,
    /// The link to the resolver task. `None` leaves every `remote_host` as `NotRequested`.
    rdns: Option<Rdns>,
    /// Set by `helper_owners`.
    helper: Option<HelperOwners>,
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
            host_cookies: RefCell::new(Arc::from([])),
            rdns: None,
            helper: None,
        }
    }

    /// Connects the rebuild to `handle`. Run the returned `SocketResolve` next to
    /// `source::run`, as `run` does.
    pub fn helper_owners(&mut self, handle: HelperHandle) -> SocketResolve {
        let store = Arc::new(Mutex::new(FxHashMap::default()));
        let (wanted_tx, wanted) = watch::channel(Arc::from([]));
        self.helper = Some(HelperOwners {
            handle: handle.clone(),
            wanted: wanted_tx,
            wanted_sent: false,
            store: Arc::clone(&store),
            uids: FxHashSet::default(),
            inodes: Vec::new(),
        });
        SocketResolve {
            handle,
            wanted,
            store,
            cursor: None,
        }
    }

    /// Fills `remote_host` through the resolver task behind `rdns`.
    pub fn with_rdns(mut self, rdns: Rdns) -> Self {
        self.rdns = Some(rdns);
        self
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
        if let Some(helper) = &mut self.helper {
            helper.after_rebuild(&self.socks, &mut self.map);
        }

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

        let mut records = if tab_shown {
            let ifnames = self.reader.if_names();
            self.table.update(&self.socks, &self.map, &ifnames);
            self.table.records.iter().map(to_model).collect()
        } else {
            Vec::new()
        };
        self.fill_hosts(&mut records, clock.boot_ns);

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

    /// Takes the resolver's answers, then sets `remote_host` on the visible Remote host cells
    /// from the cache and asks for the addresses it lacks. Without visible cells it asks for
    /// nothing and forgets what it asked for, as the resolver drops its queue then too.
    fn fill_hosts(&mut self, records: &mut [ConnRecord], now_ns: u64) {
        let Some(rdns) = &mut self.rdns else {
            return;
        };
        rdns.take_answers(now_ns);
        let cookies = self.host_cookies.borrow();
        if cookies.is_empty() {
            rdns.pending.clear();
            return;
        }
        for cookie in cookies.iter() {
            // `ConnectionTable` keeps the records sorted by cookie.
            let Ok(i) = records.binary_search_by_key(cookie, |r| r.cookie) else {
                continue;
            };
            let ip = records[i].four_tuple.remote.ip();
            if !ip.is_unspecified() {
                records[i].remote_host = rdns.host(ip, now_ns);
            }
        }
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
        if !Arc::ptr_eq(&self.host_cookies.borrow(), &d.remote_host_cookies) {
            *self.host_cookies.borrow_mut() = Arc::clone(&d.remote_host_cookies);
        }
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

/// `source::run` for the rebuilds, with `SocketResolve` beside it when the session has a helper.
pub async fn run<R: SocketReader>(
    cx: Cx,
    io: WorkerIo,
    mut src: ConnectionsSource<R>,
) -> Outcome<(), WorkerError> {
    let Some(handle) = io.shared.helper.clone() else {
        return source::run(cx, io, src).await;
    };
    let resolve = src.helper_owners(handle);
    let helper_cx = cx.clone();
    let rebuilds = source::run(cx, io, src);
    let helper = async move {
        resolve.run(&helper_cx).await;
        // Only a cancel ends the requests; the rebuild loop returns for it.
        std::future::pending().await
    };
    futures_lite::future::or(rebuilds, helper).await
}

// ---- Helper socket owners (E29-018, SYS-030, PROC-032) ----

/// The most inodes in one `ResolveSocketInodes` request. The rest wait for the next rebuild.
pub const RESOLVE_MAX: usize = 4096;
/// The deadline of one `ResolveSocketInodes` call.
const RESOLVE_DEADLINE: Duration = Duration::from_secs(2);

/// The owners the helper found, by socket inode.
type OwnerStore = Arc<Mutex<FxHashMap<u64, SmallVec<[ProcessKey; 1]>>>>;

fn lock_store(
    store: &OwnerStore,
) -> std::sync::MutexGuard<'_, FxHashMap<u64, SmallVec<[ProcessKey; 1]>>> {
    store.lock().unwrap_or_else(PoisonError::into_inner)
}

/// The rebuild's side of the helper requests.
struct HelperOwners {
    handle: HelperHandle,
    /// The sorted inodes of the last rebuild whose owner is Restricted.
    wanted: watch::Sender<Arc<[u64]>>,
    /// The last value sent on `wanted` was not empty.
    wanted_sent: bool,
    store: OwnerStore,
    /// Scratch: the uids of the restricted pids.
    uids: FxHashSet<u32>,
    /// Scratch: the inodes to send.
    inodes: Vec<u64>,
}

impl HelperOwners {
    /// Sends the inodes `map` has no owner for to `SocketResolve`, then merges the owners the
    /// helper found for them into `map`. While the helper is not `Unlocked`, nothing is sent or
    /// merged and the found owners are dropped.
    fn after_rebuild(&mut self, socks: &[SockRecord], map: &mut SocketMap) {
        if self.handle.state() != HelperState::Unlocked {
            if self.wanted_sent {
                let _ = self.wanted.send(Arc::from([]));
                self.wanted_sent = false;
                lock_store(&self.store).clear();
            }
            return;
        }
        // The same test as the Restricted owner in `ConnectionTable`: no owner in the map and
        // the uid of a pid whose fd directory could not be read. Inode 0 and TIME-WAIT stay
        // "No owner (kernel)".
        self.uids.clear();
        self.uids.extend(map.restricted.values().copied());
        self.inodes.clear();
        if !self.uids.is_empty() {
            self.inodes.extend(
                socks
                    .iter()
                    .filter(|s| {
                        s.inode != 0
                            && !(s.proto == sock_diag::Proto::Tcp && s.state == TCP_TIME_WAIT)
                            && self.uids.contains(&s.uid)
                            && map.owners_of(s.inode).is_empty()
                    })
                    .map(|s| s.inode),
            );
            self.inodes.sort_unstable();
            self.inodes.dedup();
        }
        self.wanted_sent = !self.inodes.is_empty();
        // `send` fails only when `SocketResolve` is gone.
        let _ = self.wanted.send(Arc::from(&self.inodes[..]));

        let mut store = lock_store(&self.store);
        let inodes = &self.inodes;
        store.retain(|inode, _| inodes.binary_search(inode).is_ok());
        for (&inode, keys) in store.iter() {
            map.owners.insert(inode, keys.clone());
        }
    }
}

/// The helper side: after a rebuild, one `ResolveSocketInodes` with at most `RESOLVE_MAX` of
/// its inodes. A request still running when the next rebuild ends delays it, so only one is
/// ever outstanding (SYS-030).
pub struct SocketResolve {
    handle: HelperHandle,
    wanted: watch::Receiver<Arc<[u64]>>,
    store: OwnerStore,
    /// The last inode sent: the next request starts after it, so inodes past the cap wait
    /// their turn in inode order.
    cursor: Option<u64>,
}

impl SocketResolve {
    /// Runs until `cx` is cancelled or the rebuild side is dropped.
    pub async fn run(mut self, cx: &Cx) {
        while self.wanted.changed(cx).await.is_ok() {
            if cx.checkpoint().is_err() {
                return;
            }
            if self.handle.state() != HelperState::Unlocked {
                continue;
            }
            let batch = self.pick();
            if batch.is_empty() {
                continue;
            }
            let req = HelperRequest::ResolveSocketInodes {
                inodes: batch.clone(),
            };
            // `read` never starts a session: requests run only after the user unlocked, and a
            // helper that exits meanwhile gives `NotRunning`, never a polkit prompt.
            match self.handle.read(cx, req, RESOLVE_DEADLINE).await {
                Ok(replies) => self.store(&batch, &replies),
                Err(HelperReadError::Call(HelperCallError::Cancelled)) => return,
                // The owners stay "?".
                Err(_) => {}
            }
        }
    }

    /// The wanted inodes after `cursor`, wrapping round, at most `RESOLVE_MAX`, sorted.
    fn pick(&mut self) -> Vec<u64> {
        let wanted = self.wanted.borrow_and_clone();
        let start = self
            .cursor
            .map_or(0, |c| wanted.partition_point(|&inode| inode <= c));
        let mut batch: Vec<u64> = wanted[start..]
            .iter()
            .chain(&wanted[..start])
            .take(RESOLVE_MAX)
            .copied()
            .collect();
        self.cursor = (wanted.len() > RESOLVE_MAX)
            .then(|| batch.last().copied())
            .flatten();
        batch.sort_unstable();
        batch
    }

    /// Keeps the owners of one reply for the next rebuild. An inode of `batch` the helper did
    /// not resolve loses its old owners; an error reply changes nothing.
    fn store(&self, batch: &[u64], replies: &[HelperReply]) {
        let mut found: Vec<(u64, ProcessKey)> = Vec::new();
        for reply in replies {
            match &reply.body {
                HelperReplyBody::SocketOwners(pairs) => found.extend(pairs),
                _ => return,
            }
        }
        let mut store = lock_store(&self.store);
        for inode in batch {
            store.remove(inode);
        }
        for (inode, key) in found {
            // Only inodes that were asked for.
            if batch.binary_search(&inode).is_ok() {
                store.entry(inode).or_default().push(key);
            }
        }
        for keys in store.values_mut() {
            keys.sort_unstable();
            keys.dedup();
        }
    }
}

/// A message to the resolver task.
pub enum RdnsMsg {
    /// From the source: look up this address unless it is queued or running already.
    Lookup(IpAddr),
    /// From a finished lookup task.
    Done(IpAddr),
}

/// An address and its name; `None` for no name, an error or a timeout.
pub type RdnsAnswer = (IpAddr, Option<Arc<str>>);

/// The source's half of reverse DNS: the cache and the channels to the resolver task.
pub struct Rdns {
    requests: mpsc::Sender<RdnsMsg>,
    answers: mpsc::Receiver<RdnsAnswer>,
    /// Answers by address (not by cookie), with the boot time in ns when each one expires.
    cache: FxHashMap<IpAddr, (Option<Arc<str>>, u64)>,
    /// Addresses sent to the resolver and not answered yet, with the boot time in ns sent.
    pending: FxHashMap<IpAddr, u64>,
}

impl Rdns {
    pub fn new(requests: mpsc::Sender<RdnsMsg>, answers: mpsc::Receiver<RdnsAnswer>) -> Self {
        Self {
            requests,
            answers,
            cache: FxHashMap::default(),
            pending: FxHashMap::default(),
        }
    }

    /// Caches every answer that has arrived, also those for cells no longer shown.
    fn take_answers(&mut self, now_ns: u64) {
        while let Ok((ip, name)) = self.answers.try_recv() {
            self.pending.remove(&ip);
            let ttl = if name.is_some() {
                POSITIVE_TTL
            } else {
                NEGATIVE_TTL
            };
            self.insert(ip, name, now_ns, ttl);
        }
    }

    /// When the cache is full, the expired entries go first, then the one that expires first.
    fn insert(&mut self, ip: IpAddr, name: Option<Arc<str>>, now_ns: u64, ttl: Duration) {
        if self.cache.len() >= CACHE_MAX && !self.cache.contains_key(&ip) {
            self.cache.retain(|_, (_, expires)| *expires > now_ns);
            if self.cache.len() >= CACHE_MAX
                && let Some(first) = self
                    .cache
                    .iter()
                    .min_by_key(|(_, (_, expires))| *expires)
                    .map(|(ip, _)| *ip)
            {
                self.cache.remove(&first);
            }
        }
        let expires = now_ns.saturating_add(duration_ns(ttl));
        self.cache.insert(ip, (name, expires));
    }

    /// The state of the lookup for `ip`: `Name` or `NoName` from the cache, else `Pending`. An
    /// address that is not cached, or whose entry expired, is sent to the resolver.
    fn host(&mut self, ip: IpAddr, now_ns: u64) -> RemoteHost {
        match self.cache.get(&ip) {
            Some((name, expires)) if *expires > now_ns => {
                return name.clone().map_or(RemoteHost::NoName, RemoteHost::Name);
            }
            Some(_) => {
                self.cache.remove(&ip);
            }
            None => {}
        }
        let ask = match self.pending.get(&ip) {
            Some(&sent) => now_ns.saturating_sub(sent) >= duration_ns(PENDING_STALE),
            None => self.pending.len() < CACHE_MAX,
        };
        if ask && self.requests.try_send(RdnsMsg::Lookup(ip)).is_ok() {
            self.pending.insert(ip, now_ns);
        }
        RemoteHost::Pending
    }
}

fn duration_ns(d: Duration) -> u64 {
    u64::try_from(d.as_nanos()).unwrap_or(u64::MAX)
}

/// Reverse DNS for one address. The real engine runs `tl_ops::sys::reverse_lookup` on the
/// blocking pool; tests answer from a script after a delay in virtual time.
pub trait Resolve: Send + Sync + 'static {
    /// The name of `ip`; `None` for no name and for any error (errors count as negative
    /// answers).
    fn lookup(&self, cx: &Cx, ip: IpAddr) -> impl Future<Output = Option<Arc<str>>> + Send;
}

/// `getnameinfo` through `tl_ops::sys::reverse_lookup`, one blocking job per lookup.
pub struct HostResolver;

impl Resolve for HostResolver {
    fn lookup(&self, cx: &Cx, ip: IpAddr) -> impl Future<Output = Option<Arc<str>>> + Send {
        let job = cx.spawn_blocking(move |_| tl_ops::sys::reverse_lookup(ip));
        async move {
            let mut job = job.ok()?;
            match job.join(cx).await {
                Ok(Ok(name)) => name.map(Arc::from),
                Ok(Err(errno)) => {
                    tracing::debug!(errno = errno.raw_os_error(), "reverse lookup failed");
                    None
                }
                Err(_) => None,
            }
        }
    }
}

/// The resolver task: a FIFO queue of addresses and at most `MAX_LOOKUPS` lookup tasks at
/// once, each with `LOOKUP_TIMEOUT`. It wakes on a request or a finished lookup. While no
/// Remote host cell is visible it drops its queue and starts nothing; running lookups still
/// send their answers. Runs until its region is cancelled.
pub async fn run_resolver<R: Resolve>(
    cx: Cx,
    resolver: Arc<R>,
    mut inbox: mpsc::Receiver<RdnsMsg>,
    done: mpsc::Sender<RdnsMsg>,
    answers: mpsc::Sender<RdnsAnswer>,
    demand: watch::Receiver<Demand>,
) {
    let mut queue = VecDeque::new();
    // Queued or running, so an address is never looked up twice at once.
    let mut known = FxHashSet::default();
    let mut running = 0usize;
    loop {
        let Ok(first) = inbox.recv(&cx).await else {
            return;
        };
        let mut next = Some(first);
        while let Some(msg) = next {
            match msg {
                RdnsMsg::Lookup(ip) => {
                    if known.insert(ip) {
                        queue.push_back(ip);
                    }
                }
                RdnsMsg::Done(ip) => {
                    running -= 1;
                    known.remove(&ip);
                }
            }
            next = inbox.try_recv().ok();
        }
        if demand.borrow().remote_host_cookies.is_empty() {
            for ip in queue.drain(..) {
                known.remove(&ip);
            }
            continue;
        }
        while running < MAX_LOOKUPS
            && let Some(ip) = queue.pop_front()
        {
            let resolver = Arc::clone(&resolver);
            let task_answers = answers.clone();
            let task_done = done.clone();
            let spawned = cx.spawn(move |cx| async move {
                let found = time::timeout(cx.now(), LOOKUP_TIMEOUT, resolver.lookup(&cx, ip)).await;
                if cx.checkpoint().is_err() {
                    return;
                }
                let _ = task_answers.try_send((ip, found.ok().flatten()));
                let _ = task_done.send(&cx, RdnsMsg::Done(ip)).await;
            });
            if spawned.is_ok() {
                running += 1;
            } else {
                known.remove(&ip);
                let _ = answers.try_send((ip, None));
            }
        }
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
        interface: r.interface.clone(),
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
        remote_host: RemoteHost::NotRequested,
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
            let (requests, inbox) = mpsc::channel(REQUEST_CAPACITY);
            let (answer_tx, answers) = mpsc::channel(ANSWER_CAPACITY);
            let done = requests.clone();
            let demand = io.demand.clone();
            let resolver = match cx.spawn(move |cx| {
                run_resolver(cx, Arc::new(HostResolver), inbox, done, answer_tx, demand)
            }) {
                Ok(task) => task,
                Err(e) => return Outcome::Err(WorkerError::Other(format!("spawn: {e}").into())),
            };
            let src = ConnectionsSource::new(HostReader::new(root, boot))
                .with_rdns(Rdns::new(requests, answers));
            let outcome = run(cx, io, src).await;
            resolver.abort();
            outcome
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
        ifnames: IfNames,
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
            self.ifnames.clone()
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
    fn interface_name_is_resolved_before_model_publish() {
        let mut host = tcp(7, 55, 0, 0);
        host.ifindex = 2;
        let zero = tcp(8, 56, 0, 0);
        let fake = Fake {
            tcp: vec![host.clone(), zero],
            ifnames: IfNames([(2, Arc::from("eth0"))].into_iter().collect()),
            ..Fake::default()
        };
        let src = ConnectionsSource::new(fake);
        let (_, update) = sample(src, &tab(), at(10, 10));
        assert_eq!(update.records[0].interface.as_deref(), Some("eth0"));
        assert_eq!(update.records[1].interface, None);

        // A table built for another network namespace does not receive the host name map.
        let mut table = ConnectionTable::new();
        table.update(&[host], &SocketMap::new(), &IfNames::default());
        assert_eq!(to_model(&table.records[0]).interface, None);
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

    /// Answers from `names` after `delay` of virtual time, and counts the lookups.
    #[derive(Default)]
    struct FakeResolver {
        delay: Duration,
        names: FxHashMap<IpAddr, &'static str>,
        calls: AtomicU32,
        running: AtomicU32,
        max_running: AtomicU32,
    }

    impl Resolve for FakeResolver {
        async fn lookup(&self, cx: &Cx, ip: IpAddr) -> Option<Arc<str>> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            let now = self.running.fetch_add(1, Ordering::SeqCst) + 1;
            self.max_running.fetch_max(now, Ordering::SeqCst);
            if !self.delay.is_zero() {
                asupersync::time::sleep(cx.now(), self.delay).await;
            }
            self.running.fetch_sub(1, Ordering::SeqCst);
            self.names.get(&ip).map(|&name| Arc::from(name))
        }
    }

    /// The resolver task in a lab, fed by `requests`; `rdns` is the source's half.
    struct RdnsRig {
        lab: LabRuntime,
        fake: Arc<FakeResolver>,
        demand_tx: watch::Sender<Demand>,
        requests: mpsc::Sender<RdnsMsg>,
        rdns: Option<Rdns>,
    }

    impl RdnsRig {
        fn new(fake: FakeResolver, demand: Demand) -> Self {
            let mut lab = LabRuntime::new(LabConfig::new(7).max_steps(10_000_000));
            let root = lab.state.create_root_region(Budget::INFINITE);
            let fake = Arc::new(fake);
            let (demand_tx, demand_rx) = watch::channel(demand);
            let (requests, inbox) = mpsc::channel(REQUEST_CAPACITY);
            let (answer_tx, answers) = mpsc::channel(ANSWER_CAPACITY);
            let done = requests.clone();
            let resolver = Arc::clone(&fake);
            let (task, _handle) = lab
                .state
                .create_task(root, Budget::INFINITE, async move {
                    let cx = Cx::current().expect("task has a Cx");
                    run_resolver(cx, resolver, inbox, done, answer_tx, demand_rx).await;
                })
                .expect("task is created");
            lab.scheduler.lock().schedule(task, 0);
            lab.run_until_idle();
            let rdns = Some(Rdns::new(requests.clone(), answers));
            Self {
                lab,
                fake,
                demand_tx,
                requests,
                rdns,
            }
        }

        fn advance(&mut self, d: Duration) {
            self.lab.advance_time(duration_ns(d));
            // `run_until_idle` stops when no task is ready; one step fires the due timers.
            self.lab.step_for_test();
            self.lab.run_until_idle();
        }

        fn calls(&self) -> u32 {
            self.fake.calls.load(Ordering::SeqCst)
        }

        fn lookup(&mut self, ip: IpAddr) {
            assert!(self.requests.try_send(RdnsMsg::Lookup(ip)).is_ok());
        }

        fn answers(&mut self) -> usize {
            let rdns = self.rdns.as_mut().expect("rdns");
            std::iter::from_fn(|| rdns.answers.try_recv().ok()).count()
        }
    }

    fn addr(i: u8) -> IpAddr {
        IpAddr::V4(Ipv4Addr::new(192, 0, 2, i))
    }

    fn host_demand(cookies: &[u64]) -> Demand {
        Demand {
            connections: true,
            remote_host_cookies: Arc::from(cookies),
            ..Demand::default()
        }
    }

    #[test]
    fn rdns_max_four_in_flight() {
        let fake = FakeResolver {
            delay: Duration::from_secs(1),
            ..FakeResolver::default()
        };
        let mut rig = RdnsRig::new(fake, host_demand(&[1]));
        for i in 1..=10 {
            rig.lookup(addr(i));
        }
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 4);
        assert_eq!(rig.fake.running.load(Ordering::SeqCst), 4);

        rig.advance(Duration::from_secs(1));
        assert_eq!(rig.calls(), 8);
        rig.advance(Duration::from_secs(1));
        assert_eq!(rig.calls(), 10);
        rig.advance(Duration::from_secs(1));
        assert_eq!(rig.fake.max_running.load(Ordering::SeqCst), 4);
        assert_eq!(rig.answers(), 10);
    }

    /// A source with one TCP record (cookie 7, foreign 10.0.0.9) and the rig's `rdns`.
    fn host_source(rig: &mut RdnsRig) -> ConnectionsSource<Fake> {
        let fake = Fake {
            tcp: vec![tcp(7, 55, 0, 0)],
            owners: vec![(55, vec![key(100)])],
            ..Fake::default()
        };
        ConnectionsSource::new(fake).with_rdns(rig.rdns.take().expect("rdns"))
    }

    const FOREIGN: IpAddr = IpAddr::V4(Ipv4Addr::new(10, 0, 0, 9));

    #[test]
    fn rdns_positive_ttl_10_min() {
        let fake = FakeResolver {
            names: [(FOREIGN, "host.example")].into_iter().collect(),
            ..FakeResolver::default()
        };
        let d = host_demand(&[7]);
        let mut rig = RdnsRig::new(fake, d.clone());
        let src = host_source(&mut rig);

        // Pending: the cell shows the address.
        let (src, update) = sample(src, &d, at(100, 100));
        assert_eq!(update.records[0].remote_host, RemoteHost::Pending);
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 1);

        // Answered and cached at t=100 s.
        let (src, update) = sample(src, &d, at(100, 100));
        assert_eq!(
            update.records[0].remote_host,
            RemoteHost::Name(Arc::from("host.example"))
        );

        let (src, update) = sample(src, &d, at(100 + 9 * 60, 100 + 9 * 60));
        assert_eq!(
            update.records[0].remote_host,
            RemoteHost::Name(Arc::from("host.example"))
        );
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 1, "no new lookup at 9 min");

        let (_, update) = sample(src, &d, at(100 + 11 * 60, 100 + 11 * 60));
        assert_eq!(update.records[0].remote_host, RemoteHost::Pending);
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 2, "a new lookup at 11 min");
    }

    #[test]
    fn rdns_not_requested_without_cookie() {
        // The cookie is not in `remote_host_cookies`, so the record is never looked up.
        let d = host_demand(&[8]);
        let mut rig = RdnsRig::new(FakeResolver::default(), d.clone());
        let src = host_source(&mut rig);

        let (_, update) = sample(src, &d, at(100, 100));
        assert_eq!(update.records[0].remote_host, RemoteHost::NotRequested);
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 0);
    }

    #[test]
    fn rdns_negative_ttl_1_min() {
        let d = host_demand(&[7]);
        let mut rig = RdnsRig::new(FakeResolver::default(), d.clone());
        let src = host_source(&mut rig);

        let (src, update) = sample(src, &d, at(100, 100));
        assert_eq!(update.records[0].remote_host, RemoteHost::Pending);
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 1);
        // The negative answer is cached at t=100 s.
        let (src, update) = sample(src, &d, at(100, 100));
        assert_eq!(update.records[0].remote_host, RemoteHost::NoName);

        let (src, _) = sample(src, &d, at(130, 130));
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 1, "no new lookup at 30 s");

        let (_, _) = sample(src, &d, at(161, 161));
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 2, "a new lookup at 61 s");
    }

    #[test]
    fn rdns_queue_cleared_without_demand() {
        let fake = FakeResolver {
            delay: Duration::from_secs(1),
            ..FakeResolver::default()
        };
        let mut rig = RdnsRig::new(fake, host_demand(&[1]));
        for i in 1..=10 {
            rig.lookup(addr(i));
        }
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 4);

        // The column is hidden while 4 lookups run and 6 wait.
        rig.demand_tx
            .send(host_demand(&[]))
            .expect("watch has receivers");
        rig.advance(Duration::from_secs(1));
        for _ in 0..10 {
            rig.advance(Duration::from_secs(1));
        }
        assert_eq!(
            rig.calls(),
            4,
            "no lookup starts after the queue is dropped"
        );
        // The finished lookups still answer, so the source can cache them.
        assert_eq!(rig.answers(), 4);

        // The source asks for nothing without visible cells.
        let fake = Fake {
            tcp: vec![tcp(7, 55, 0, 0)],
            ..Fake::default()
        };
        let mut rdns = rig.rdns.take().expect("rdns");
        rdns.pending.insert(FOREIGN, 0);
        let src = ConnectionsSource::new(fake).with_rdns(rdns);
        let (src, _) = sample(src, &tab(), at(100, 100));
        assert!(src.rdns.as_ref().expect("rdns").pending.is_empty());
        rig.lab.run_until_idle();
        assert_eq!(rig.calls(), 4);
    }
}
