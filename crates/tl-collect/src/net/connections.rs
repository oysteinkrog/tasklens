//! Connection records for the Connections tab (SYS-028 to SYS-030, ADR-003 `ConnectionsSnapshot`,
//! canonical table d "Addresses").
//!
//! One record per socket, keyed by socket cookie. Address strings are formatted once per cookie
//! and reused while the endpoints stay the same. Addresses are never logged.

use std::fmt::Write as _;
use std::net::IpAddr;
use std::sync::Arc;

use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::SmallVec;
use tl_model::ids::ProcessKey;

use crate::CollectError;
use crate::net::sock_diag::{Proto, SockRecord};
use crate::net::socket_map::SocketMap;
use crate::sysroot::{SysRoot, read_u64};

const TCP_ESTABLISHED: u8 = 1;
const TCP_TIME_WAIT: u8 = 6;
const UDP_UNCONNECTED: u8 = 7;
const TCP_LISTEN: u8 = 10;

/// Who holds a socket.
#[derive(Clone, Debug, PartialEq)]
pub enum Owner {
    /// Inode 0 or TCP TIME-WAIT: PID "-", Process "No owner (kernel)".
    Kernel,
    /// One or more owners. Empty when the owner exited between the dump and the walk (shown "-").
    Processes(SmallVec<[ProcessKey; 1]>),
    /// No owner found and processes of the socket's uid were unreadable: PID "?".
    Restricted { uid: u32 },
}

#[derive(Clone, Debug)]
pub struct ConnRecord {
    /// Row identity.
    pub cookie: u64,
    pub sock: SockRecord,
    /// Formatted local address with port.
    pub local: Arc<str>,
    /// Formatted foreign address with port.
    pub foreign: Arc<str>,
    /// Name for `sock.ifindex`, `None` when 0 or unknown.
    pub interface: Option<Arc<str>>,
    pub owner: Owner,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ConnCounts {
    pub total: u32,
    pub tcp: u32,
    pub udp: u32,
    /// TCP rows in state 1 only.
    pub established: u32,
    /// TCP rows in state 10 only.
    pub listening: u32,
}

/// Endpoints a cached address pair was formatted from.
#[derive(Clone, PartialEq, Eq)]
struct EndpointKey {
    src: IpAddr,
    sport: u16,
    dst: IpAddr,
    dport: u16,
    ifindex: u32,
    foreign_unbound: bool,
}

struct CachedAddrs {
    key: EndpointKey,
    local: Arc<str>,
    foreign: Arc<str>,
}

pub struct ConnectionTable {
    /// Sorted by cookie.
    pub records: Vec<ConnRecord>,
    pub counts: ConnCounts,
    /// cookie -> address strings from the last update.
    cache: FxHashMap<u64, CachedAddrs>,
    /// Cache being built by the current update, swapped into `cache` at the end.
    next_cache: FxHashMap<u64, CachedAddrs>,
    /// uids of the restricted pids in the current map.
    restricted_uids: FxHashSet<u32>,
    scratch: String,
    scope: String,
}

impl Default for ConnectionTable {
    fn default() -> Self {
        Self::new()
    }
}

impl ConnectionTable {
    pub fn new() -> Self {
        Self {
            records: Vec::new(),
            counts: ConnCounts::default(),
            cache: FxHashMap::default(),
            next_cache: FxHashMap::default(),
            restricted_uids: FxHashSet::default(),
            scratch: String::new(),
            scope: String::new(),
        }
    }

    /// Rebuilds from one `dump_all`. Address strings of cookies seen last time with the same
    /// (src, sport, dst, dport, ifindex) are reused (Arc clone), not re-formatted.
    pub fn update(&mut self, socks: &[SockRecord], map: &SocketMap, ifnames: &IfNames) {
        self.records.clear();
        self.next_cache.clear();
        self.counts = ConnCounts::default();
        self.restricted_uids.clear();
        self.restricted_uids
            .extend(map.restricted.values().copied());

        for sock in socks {
            let key = EndpointKey {
                src: sock.src,
                sport: sock.sport,
                dst: sock.dst,
                dport: sock.dport,
                ifindex: sock.ifindex,
                foreign_unbound: foreign_unbound(sock),
            };
            let (local, foreign) = match self.cache.get(&sock.cookie) {
                Some(c) if c.key == key => (c.local.clone(), c.foreign.clone()),
                _ => self.format_pair(&key, ifnames),
            };
            self.next_cache.insert(
                sock.cookie,
                CachedAddrs {
                    key,
                    local: local.clone(),
                    foreign: foreign.clone(),
                },
            );

            self.counts.total += 1;
            match sock.proto {
                Proto::Tcp => {
                    self.counts.tcp += 1;
                    match sock.state {
                        TCP_ESTABLISHED => self.counts.established += 1,
                        TCP_LISTEN => self.counts.listening += 1,
                        _ => {}
                    }
                }
                Proto::Udp => self.counts.udp += 1,
            }

            let interface = match sock.ifindex {
                0 => None,
                i => ifnames.0.get(&i).cloned(),
            };
            self.records.push(ConnRecord {
                cookie: sock.cookie,
                sock: sock.clone(),
                local,
                foreign,
                interface,
                owner: resolve_owner(sock, map, &self.restricted_uids),
            });
        }
        self.records.sort_unstable_by_key(|r| r.cookie);
        std::mem::swap(&mut self.cache, &mut self.next_cache);
    }

    fn format_pair(&mut self, key: &EndpointKey, ifnames: &IfNames) -> (Arc<str>, Arc<str>) {
        self.scratch.clear();
        let scope = scope_for(key.src, key.ifindex, ifnames, &mut self.scope);
        format_endpoint(key.src, key.sport, scope, false, &mut self.scratch);
        let local = Arc::from(self.scratch.as_str());

        self.scratch.clear();
        let scope = scope_for(key.dst, key.ifindex, ifnames, &mut self.scope);
        format_endpoint(
            key.dst,
            key.dport,
            scope,
            key.foreign_unbound,
            &mut self.scratch,
        );
        (local, Arc::from(self.scratch.as_str()))
    }
}

/// A TCP LISTEN socket, or a UDP socket in state 7 with an all-zero destination.
fn foreign_unbound(sock: &SockRecord) -> bool {
    match sock.proto {
        Proto::Tcp => sock.state == TCP_LISTEN,
        Proto::Udp => sock.state == UDP_UNCONNECTED && sock.dst.is_unspecified(),
    }
}

fn resolve_owner(sock: &SockRecord, map: &SocketMap, restricted_uids: &FxHashSet<u32>) -> Owner {
    if sock.inode == 0 || (sock.proto == Proto::Tcp && sock.state == TCP_TIME_WAIT) {
        return Owner::Kernel;
    }
    let owners = map.owners_of(sock.inode);
    if !owners.is_empty() {
        return Owner::Processes(owners.iter().copied().collect());
    }
    if restricted_uids.contains(&sock.uid) {
        return Owner::Restricted { uid: sock.uid };
    }
    Owner::Processes(SmallVec::new())
}

/// The scope for a link-local IPv6 address: the interface name, or the decimal ifindex when the
/// name is unknown. `None` for any other address and for ifindex 0.
fn scope_for<'a>(
    addr: IpAddr,
    ifindex: u32,
    ifnames: &'a IfNames,
    buf: &'a mut String,
) -> Option<&'a str> {
    let IpAddr::V6(v6) = addr else { return None };
    if ifindex == 0 || !v6.is_unicast_link_local() {
        return None;
    }
    if let Some(name) = ifnames.0.get(&ifindex) {
        return Some(name);
    }
    buf.clear();
    let _ = write!(buf, "{ifindex}");
    Some(buf)
}

/// ifindex -> interface name, read from /sys/class/net/<name>/ifindex.
#[derive(Clone, Debug, Default)]
pub struct IfNames(pub FxHashMap<u32, Arc<str>>);

impl IfNames {
    /// Fails only when `/sys/class/net` cannot be listed. An interface whose name is not UTF-8
    /// or whose `ifindex` cannot be read is left out: it may have gone away during the scan.
    pub fn read(root: &SysRoot) -> Result<Self, CollectError> {
        let class = root.sys_path("class/net");
        let mut names = FxHashMap::default();
        let mut buf = Vec::new();
        for entry in std::fs::read_dir(&class)? {
            let entry = entry?;
            let Ok(name) = entry.file_name().into_string() else {
                continue;
            };
            let Ok(ifindex) = read_u64(&class.join(&name).join("ifindex"), &mut buf) else {
                continue;
            };
            let Ok(ifindex) = u32::try_from(ifindex) else {
                continue;
            };
            names.insert(ifindex, Arc::from(name));
        }
        Ok(Self(names))
    }
}

/// Appends one endpoint to `out` (table d, SYS-029).
///
/// IPv4 is `a.b.c.d:port`. IPv6 is `[addr]:port` in RFC 5952 form, with `%scope` inside the
/// brackets when `scope` is given. An IPv4-mapped IPv6 address is written in IPv4 form. Port 0 is
/// `*`. `foreign_unbound` writes `*:*` whatever the address.
pub fn format_endpoint(
    addr: IpAddr,
    port: u16,
    scope: Option<&str>,
    foreign_unbound: bool,
    out: &mut String,
) {
    if foreign_unbound {
        out.push_str("*:*");
        return;
    }
    // Writing to a String cannot fail.
    let _ = match addr {
        IpAddr::V4(v4) => write!(out, "{v4}"),
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => write!(out, "{v4}"),
            None => match scope {
                Some(scope) => write!(out, "[{v6}%{scope}]"),
                None => write!(out, "[{v6}]"),
            },
        },
    };
    out.push(':');
    if port == 0 {
        out.push('*');
    } else {
        let _ = write!(out, "{port}");
    }
}

#[cfg(test)]
mod tests {
    use std::net::{Ipv4Addr, Ipv6Addr};
    use std::path::Path;

    use tl_model::time::BootId;

    use super::*;
    use crate::net::sock_diag::Family;

    fn sock(proto: Proto, state: u8, cookie: u64, inode: u64) -> SockRecord {
        SockRecord {
            family: Family::V4,
            proto,
            state,
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
            tcp_info: None,
        }
    }

    fn key(pid: u32) -> ProcessKey {
        ProcessKey {
            start_ticks: u64::from(pid) * 10,
            boot_id: BootId(1),
            pid,
        }
    }

    fn one_record(s: SockRecord, map: &SocketMap, ifnames: &IfNames) -> ConnRecord {
        let mut table = ConnectionTable::new();
        table.update(&[s], map, ifnames);
        table.records.pop().unwrap()
    }

    fn endpoint(addr: IpAddr, port: u16, scope: Option<&str>) -> String {
        let mut out = String::new();
        format_endpoint(addr, port, scope, false, &mut out);
        out
    }

    #[test]
    fn tcp4_listener_format() {
        let mut s = sock(Proto::Tcp, TCP_LISTEN, 1, 77);
        s.src = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        s.dst = IpAddr::V4(Ipv4Addr::UNSPECIFIED);
        s.sport = 22;
        s.dport = 0;
        let r = one_record(s, &SocketMap::new(), &IfNames::default());
        assert_eq!(&*r.local, "0.0.0.0:22");
        assert_eq!(&*r.foreign, "*:*");
    }

    #[test]
    fn udp6_wildcard_format() {
        let mut s = sock(Proto::Udp, UDP_UNCONNECTED, 1, 77);
        s.family = Family::V6;
        s.src = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
        s.dst = IpAddr::V6(Ipv6Addr::UNSPECIFIED);
        s.sport = 0;
        s.dport = 0;
        let r = one_record(s, &SocketMap::new(), &IfNames::default());
        assert_eq!(&*r.local, "[::]:*");
        assert_eq!(&*r.foreign, "*:*");
    }

    #[test]
    fn v4_mapped_as_v4() {
        let addr = IpAddr::V6(Ipv4Addr::new(192, 168, 1, 5).to_ipv6_mapped());
        assert_eq!(endpoint(addr, 443, None), "192.168.1.5:443");
    }

    #[test]
    fn link_local_scope_name_and_number() {
        let ll = IpAddr::V6("fe80::1".parse().unwrap());
        let mut names = FxHashMap::default();
        names.insert(3, Arc::from("enp5s0"));
        let ifnames = IfNames(names);

        let mut s = sock(Proto::Tcp, TCP_ESTABLISHED, 1, 77);
        s.family = Family::V6;
        s.src = ll;
        s.sport = 22;
        s.ifindex = 3;
        let r = one_record(s.clone(), &SocketMap::new(), &ifnames);
        assert_eq!(&*r.local, "[fe80::1%enp5s0]:22");
        assert_eq!(r.interface.as_deref(), Some("enp5s0"));

        s.ifindex = 9;
        let r = one_record(s, &SocketMap::new(), &ifnames);
        assert_eq!(&*r.local, "[fe80::1%9]:22");
        assert_eq!(r.interface, None);

        // Other IPv6 addresses get no scope, and use the RFC 5952 form.
        let global = IpAddr::V6("2001:db8:0:0:0:0:0:1".parse().unwrap());
        assert_eq!(endpoint(global, 8080, None), "[2001:db8::1]:8080");
    }

    #[test]
    fn time_wait_owner_kernel() {
        let mut map = SocketMap::new();
        map.owners.insert(4242, [key(100)].into_iter().collect());
        let r = one_record(
            sock(Proto::Tcp, TCP_TIME_WAIT, 1, 4242),
            &map,
            &IfNames::default(),
        );
        assert_eq!(r.owner, Owner::Kernel);

        let r = one_record(
            sock(Proto::Udp, UDP_UNCONNECTED, 1, 0),
            &map,
            &IfNames::default(),
        );
        assert_eq!(r.owner, Owner::Kernel);

        let r = one_record(
            sock(Proto::Tcp, TCP_ESTABLISHED, 1, 4242),
            &map,
            &IfNames::default(),
        );
        assert_eq!(r.owner, Owner::Processes([key(100)].into_iter().collect()));
    }

    #[test]
    fn restricted_owner_by_uid() {
        let mut map = SocketMap::new();
        map.restricted.insert(1, 0);
        let mut s = sock(Proto::Tcp, TCP_LISTEN, 1, 500);
        s.uid = 0;
        let r = one_record(s.clone(), &map, &IfNames::default());
        assert_eq!(r.owner, Owner::Restricted { uid: 0 });

        // A uid with no restricted pid means the owner exited: an empty owner list.
        s.uid = 1000;
        let r = one_record(s, &map, &IfNames::default());
        assert_eq!(r.owner, Owner::Processes(SmallVec::new()));
    }

    #[test]
    fn address_strings_reused() {
        let map = SocketMap::new();
        let ifnames = IfNames::default();
        let mut table = ConnectionTable::new();
        table.update(&[sock(Proto::Tcp, TCP_ESTABLISHED, 9, 1)], &map, &ifnames);
        let local = table.records[0].local.clone();
        let foreign = table.records[0].foreign.clone();
        table.update(&[sock(Proto::Tcp, TCP_ESTABLISHED, 9, 1)], &map, &ifnames);
        assert!(Arc::ptr_eq(&local, &table.records[0].local));
        assert!(Arc::ptr_eq(&foreign, &table.records[0].foreign));

        // A changed endpoint is formatted again.
        let mut moved = sock(Proto::Tcp, TCP_ESTABLISHED, 9, 1);
        moved.dport = 8443;
        table.update(&[moved], &map, &ifnames);
        assert_eq!(*local, *table.records[0].local);
        assert_eq!(&*table.records[0].foreign, "10.0.0.9:8443");
    }

    #[test]
    fn counts_tcp_only_states() {
        let socks = [
            sock(Proto::Tcp, TCP_ESTABLISHED, 6, 1),
            sock(Proto::Tcp, TCP_ESTABLISHED, 5, 2),
            sock(Proto::Tcp, TCP_ESTABLISHED, 4, 3),
            sock(Proto::Tcp, TCP_LISTEN, 3, 4),
            sock(Proto::Udp, TCP_ESTABLISHED, 2, 5),
            sock(Proto::Udp, UDP_UNCONNECTED, 1, 6),
        ];
        let mut table = ConnectionTable::new();
        table.update(&socks, &SocketMap::new(), &IfNames::default());
        assert_eq!(
            table.counts,
            ConnCounts {
                total: 6,
                tcp: 4,
                udp: 2,
                established: 3,
                listening: 1,
            }
        );
        let cookies: Vec<u64> = table.records.iter().map(|r| r.cookie).collect();
        assert_eq!(cookies, [1, 2, 3, 4, 5, 6]);
    }

    #[test]
    fn ifnames_from_fixture() {
        let root = SysRoot::at(
            &Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/sockets-basic"),
        );
        let names = IfNames::read(&root).unwrap();
        assert_eq!(names.0.len(), 2);
        assert_eq!(names.0.get(&1).map(|n| &**n), Some("lo"));
        assert_eq!(names.0.get(&3).map(|n| &**n), Some("enp5s0"));

        let missing = SysRoot::at(Path::new("/nonexistent-tasklens-root"));
        assert!(IfNames::read(&missing).is_err());
    }
}
