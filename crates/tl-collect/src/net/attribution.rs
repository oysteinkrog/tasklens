//! Per-process TCP attribution (PROC-032, ADR-016 "Per-process network", ADR-008).
//!
//! Two sock_diag TCP dumps plus the socket map give per-process send and receive rates. Only TCP
//! sockets with `tcp_info` count. A socket held by several processes counts in full for each of
//! them. Sockets with inode 0 or no known owner count only in the unattributed and total sums.

use rustc_hash::FxHashMap;
use tl_model::ids::ProcessKey;
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::counters::Counter;
use crate::net::sock_diag::{Proto, SockRecord};
use crate::net::socket_map::SocketMap;

/// One process's TCP rates in bytes per second.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NetRate {
    pub rx_bps: f32,
    pub tx_bps: f32,
    /// At least one of the process's sockets is held by 2 or more processes.
    pub shared: bool,
    pub validity: Validity,
}

pub struct TcpAttribution {
    prev: FxHashMap<u64, (u64, u64)>, // cookie -> (bytes_acked, bytes_received) at prev_ns
    prev_ns: Option<TimeNs>,
    /// Baselines of the dump being read, swapped into `prev` at the end of `update`.
    next: FxHashMap<u64, (u64, u64)>,
    pub per_process: FxHashMap<ProcessKey, NetRate>,
    pub restricted_pids: FxHashMap<u32, u32>, // copied from SocketMap::restricted
    pub total_rx_bps: f32,
    pub total_tx_bps: f32, // every TCP socket in the dump
    pub unattributed_rx_bps: f32,
    pub unattributed_tx_bps: f32, // sockets with inode 0 or no owner
    pub validity: Validity,       // WarmingUp on the first update, else Valid
}

impl Default for TcpAttribution {
    fn default() -> Self {
        Self::new()
    }
}

impl TcpAttribution {
    pub fn new() -> Self {
        Self {
            prev: FxHashMap::default(),
            prev_ns: None,
            next: FxHashMap::default(),
            per_process: FxHashMap::default(),
            restricted_pids: FxHashMap::default(),
            total_rx_bps: 0.0,
            total_tx_bps: 0.0,
            unattributed_rx_bps: 0.0,
            unattributed_tx_bps: 0.0,
            validity: Validity::WarmingUp,
        }
    }

    /// `records` from SockDiag::dump_tcp taken at `t_ns` (CLOCK_BOOTTIME read just before the dump).
    pub fn update(&mut self, records: &[SockRecord], map: &SocketMap, t_ns: TimeNs) {
        self.per_process.clear();
        self.restricted_pids.clear();
        self.restricted_pids
            .extend(map.restricted.iter().map(|(&pid, &uid)| (pid, uid)));
        self.total_rx_bps = 0.0;
        self.total_tx_bps = 0.0;
        self.unattributed_rx_bps = 0.0;
        self.unattributed_tx_bps = 0.0;

        // No baseline, or time did not move forward: fill baselines only.
        let prev_ns = self.prev_ns.filter(|&p| t_ns > p);
        self.validity = if prev_ns.is_some() {
            Validity::Valid
        } else {
            Validity::WarmingUp
        };

        self.next.clear();
        for rec in records {
            if rec.proto != Proto::Tcp {
                continue;
            }
            let Some(info) = rec.tcp_info else { continue };
            let cur = (info.bytes_acked, info.bytes_received);
            let old = self.prev.remove(&rec.cookie);
            self.next.insert(rec.cookie, cur);
            let Some(prev_ns) = prev_ns else { continue };

            let (tx, rx) = old
                .and_then(|old| {
                    let tx = rate(old.0, cur.0, prev_ns, t_ns)?;
                    let rx = rate(old.1, cur.1, prev_ns, t_ns)?;
                    Some((tx, rx))
                })
                .unwrap_or((0.0, 0.0));
            self.total_tx_bps += tx;
            self.total_rx_bps += rx;

            let owners = if rec.inode == 0 {
                &[][..]
            } else {
                map.owners_of(rec.inode)
            };
            if owners.is_empty() {
                self.unattributed_tx_bps += tx;
                self.unattributed_rx_bps += rx;
                continue;
            }
            let shared = owners.len() >= 2;
            for &key in owners {
                let entry = self.per_process.entry(key).or_insert(NetRate {
                    rx_bps: 0.0,
                    tx_bps: 0.0,
                    shared: false,
                    validity: Validity::Valid,
                });
                entry.tx_bps += tx;
                entry.rx_bps += rx;
                entry.shared |= shared;
            }
        }
        // Cookies not in this dump are left in `prev` and dropped here.
        std::mem::swap(&mut self.prev, &mut self.next);
        self.next.clear();
        self.prev_ns = Some(t_ns);
    }

    /// Drops all baselines (suspend, epoch change, ResetBaselines). The next update is WarmingUp.
    pub fn reset(&mut self) {
        self.prev.clear();
        self.prev_ns = None;
    }
}

/// Bytes per second between two readings of one socket counter. `None` when it went backwards
/// (a reset): the socket adds nothing this interval and its new reading becomes the baseline.
fn rate(prev: u64, cur: u64, prev_ns: TimeNs, t_ns: TimeNs) -> Option<f32> {
    let mut counter = Counter::default();
    counter.step(prev, prev_ns, 0);
    let r = counter.update(cur, t_ns, 0);
    (r.validity == Validity::Valid).then_some(r.per_sec as f32)
}

#[cfg(test)]
mod tests {
    use std::net::{IpAddr, Ipv4Addr};

    use tl_model::time::BootId;

    use super::*;
    use crate::net::sock_diag::{Family, TcpInfoLite};

    const SEC: TimeNs = 1_000_000_000;

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BootId(1),
            pid,
        }
    }

    fn tcp(cookie: u64, inode: u64, acked: u64, received: u64) -> SockRecord {
        SockRecord {
            family: Family::V4,
            proto: Proto::Tcp,
            state: 1,
            src: IpAddr::V4(Ipv4Addr::LOCALHOST),
            dst: IpAddr::V4(Ipv4Addr::LOCALHOST),
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

    fn map_with(inode: u64, owners: &[ProcessKey]) -> SocketMap {
        let mut map = SocketMap::new();
        map.owners.insert(inode, owners.iter().copied().collect());
        map
    }

    #[test]
    fn rate_from_elapsed_time() {
        let a = key(100);
        let map = map_with(55, &[a]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 1000, 0)], &map, 10 * SEC);
        att.update(&[tcp(7, 55, 3000, 4000)], &map, 12 * SEC);
        assert_eq!(att.validity, Validity::Valid);
        assert_eq!(
            att.per_process[&a],
            NetRate {
                rx_bps: 2000.0,
                tx_bps: 1000.0,
                shared: false,
                validity: Validity::Valid,
            }
        );
        assert_eq!(att.total_tx_bps, 1000.0);
        assert_eq!(att.total_rx_bps, 2000.0);
        assert_eq!(att.unattributed_tx_bps, 0.0);
    }

    #[test]
    fn shared_socket_counts_for_each() {
        let (a, b) = (key(100), key(200));
        let mut map = map_with(55, &[a, b]);
        map.restricted.insert(300, 0);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 1000, 0)], &map, 10 * SEC);
        att.update(&[tcp(7, 55, 3000, 4000)], &map, 12 * SEC);
        for k in [a, b] {
            let r = att.per_process[&k];
            assert_eq!((r.tx_bps, r.rx_bps), (1000.0, 2000.0));
            assert!(r.shared);
            assert_eq!(r.validity, Validity::Valid);
        }
        assert_eq!(att.total_tx_bps, 1000.0);
        assert_eq!(att.total_rx_bps, 2000.0);
        assert_eq!(att.restricted_pids.get(&300), Some(&0));
    }

    #[test]
    fn first_update_warming_up() {
        let map = map_with(55, &[key(100)]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 1000, 500)], &map, 10 * SEC);
        assert_eq!(att.validity, Validity::WarmingUp);
        assert!(att.per_process.is_empty());
        assert_eq!(att.total_tx_bps, 0.0);
    }

    #[test]
    fn counter_reset_no_spike() {
        let a = key(100);
        let map = map_with(55, &[a]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 1000, 0)], &map, 10 * SEC);
        att.update(&[tcp(7, 55, 3000, 0)], &map, 11 * SEC);
        att.update(&[tcp(7, 55, 100, 0)], &map, 12 * SEC);
        let r = att.per_process[&a];
        assert_eq!((r.tx_bps, r.rx_bps), (0.0, 0.0));
        assert_eq!(r.validity, Validity::Valid);
        assert_eq!(att.total_tx_bps, 0.0);
        // The next interval computes from 100.
        att.update(&[tcp(7, 55, 600, 0)], &map, 13 * SEC);
        assert_eq!(att.per_process[&a].tx_bps, 500.0);
    }

    #[test]
    fn inode_zero_unattributed() {
        let a = key(100);
        // An owner listed under inode 0 must still not get the TIME-WAIT socket's traffic.
        let map = map_with(0, &[a]);
        let mut att = TcpAttribution::new();
        let mut tw = tcp(9, 0, 0, 0);
        tw.state = 6;
        att.update(std::slice::from_ref(&tw), &map, 10 * SEC);
        tw.tcp_info = Some(TcpInfoLite {
            bytes_acked: 2000,
            bytes_received: 1000,
            ..TcpInfoLite::default()
        });
        att.update(&[tw], &map, 11 * SEC);
        assert_eq!(att.unattributed_tx_bps, 2000.0);
        assert_eq!(att.unattributed_rx_bps, 1000.0);
        assert_eq!(att.total_tx_bps, 2000.0);
        assert_eq!(att.total_rx_bps, 1000.0);
        assert!(att.per_process.is_empty());
    }

    #[test]
    fn reset_restarts_warmup() {
        let a = key(100);
        let map = map_with(55, &[a]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 1000, 0)], &map, 10 * SEC);
        att.update(&[tcp(7, 55, 2000, 0)], &map, 11 * SEC);
        assert_eq!(att.validity, Validity::Valid);
        att.reset();
        att.update(&[tcp(7, 55, 3000, 0)], &map, 12 * SEC);
        assert_eq!(att.validity, Validity::WarmingUp);
        assert!(att.per_process.is_empty());
        att.update(&[tcp(7, 55, 4000, 0)], &map, 13 * SEC);
        assert_eq!(att.per_process[&a].tx_bps, 1000.0);
    }

    #[test]
    fn udp_ignored() {
        let a = key(100);
        let map = map_with(55, &[a]);
        let mut udp = tcp(8, 55, 1000, 1000);
        udp.proto = Proto::Udp;
        let mut att = TcpAttribution::new();
        att.update(std::slice::from_ref(&udp), &map, 10 * SEC);
        udp.tcp_info = Some(TcpInfoLite {
            bytes_acked: 5000,
            bytes_received: 5000,
            ..TcpInfoLite::default()
        });
        att.update(&[udp], &map, 11 * SEC);
        assert!(att.per_process.is_empty());
        assert_eq!(att.total_tx_bps, 0.0);
        assert_eq!(att.total_rx_bps, 0.0);
    }

    #[test]
    fn stale_cookies_are_dropped() {
        let map = map_with(55, &[key(100)]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 0, 0), tcp(8, 55, 0, 0)], &map, 10 * SEC);
        att.update(&[tcp(8, 55, 0, 0)], &map, 11 * SEC);
        assert_eq!(att.prev.len(), 1);
        assert!(att.prev.contains_key(&8));
    }

    #[test]
    fn idle_owner_gets_zero_entry() {
        let map = map_with(55, &[key(100)]);
        let mut att = TcpAttribution::new();
        att.update(&[tcp(7, 55, 0, 0)], &map, SEC);
        att.update(&[tcp(7, 55, 0, 0)], &map, 2 * SEC);
        let r = att.per_process[&key(100)];
        assert_eq!((r.tx_bps, r.rx_bps), (0.0, 0.0));
        assert_eq!(r.validity, Validity::Valid);
    }
}
