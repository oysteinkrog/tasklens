//! Disk eject planning and the busy-process scan (E12-022, PERF-052, ADR-016).
//!
//! `plan` orders the UDisks2 calls an eject needs: unmount every filesystem on the drive, deepest
//! first, then lock every unlocked LUKS container, then eject or power off the drive. When an
//! unmount or lock answers `DeviceBusy`, `find_blockers` scans `/proc` for the processes that
//! hold files under the drive's mount points. Paths from `/proc` are untrusted: they are compared
//! as bytes and sanitized before display, and never logged.

use std::collections::{BTreeMap, BTreeSet};
use std::ffi::OsStr;
use std::fs;
use std::io::{ErrorKind, Read};
use std::os::unix::ffi::OsStrExt;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use tl_collect::proc::pid_stat::{PidStat, parse_pid_stat};
use tl_collect::sanitize::{COMM_MAX, EXE_LINK_MAX, sanitize_text};
use tl_model::ids::ProcessKey;
use tl_model::snapshot::Blocker;
use tl_model::time::BootId;
use zbus::zvariant::{OwnedObjectPath, Value};

use crate::workers::dbus_udisks::{
    BLOCK_IFACE, DRIVE_IFACE, ManagedObjects, Props, flag, object_path,
};

pub(crate) const FILESYSTEM_IFACE: &str = "org.freedesktop.UDisks2.Filesystem";
pub(crate) const ENCRYPTED_IFACE: &str = "org.freedesktop.UDisks2.Encrypted";
/// Read limit for `/proc/<pid>/maps`.
const MAPS_MAX: u64 = 2 << 20;
/// Read limit for `/proc/<pid>/stat`.
const STAT_MAX: u64 = 4096;

/// The UDisks2 calls of one eject, in order.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EjectPlan {
    /// `Filesystem.Unmount` targets.
    pub unmount: Vec<OwnedObjectPath>,
    /// `Encrypted.Lock` targets.
    pub lock: Vec<OwnedObjectPath>,
    pub drive: OwnedObjectPath,
    /// `Drive.PowerOff` instead of `Drive.Eject`.
    pub power_off: bool,
}

/// Children of the drive in unmount order: deepest first (filesystems on dm devices, then on
/// partitions), then LUKS locks.
pub fn plan(objs: &ManagedObjects, drive: &OwnedObjectPath) -> EjectPlan {
    let members = members(objs, drive.as_str());
    let mut order: Vec<(&str, u32)> = members.into_iter().collect();
    // Deepest first, then by object path.
    order.sort_by(|(pa, da), (pb, db)| db.cmp(da).then(pa.cmp(pb)));

    let mut unmount = Vec::new();
    let mut lock = Vec::new();
    for (path, _) in order {
        let Some(ifaces) = objs.0.get(path) else {
            continue;
        };
        let Ok(owned) = OwnedObjectPath::try_from(path) else {
            continue;
        };
        if ifaces
            .get(FILESYSTEM_IFACE)
            .is_some_and(|fs| !mount_bytes(fs).is_empty())
        {
            unmount.push(owned.clone());
        }
        if ifaces
            .get(ENCRYPTED_IFACE)
            .and_then(|e| object_path(e, "CleartextDevice"))
            .is_some_and(|p| p != "/")
        {
            lock.push(owned);
        }
    }
    let drive_props = objs.0.get(drive.as_str()).and_then(|o| o.get(DRIVE_IFACE));
    let ejectable = flag(drive_props, "Ejectable").unwrap_or(false);
    let can_power_off = flag(drive_props, "CanPowerOff").unwrap_or(false);
    EjectPlan {
        unmount,
        lock,
        drive: drive.clone(),
        power_off: !ejectable && can_power_off,
    }
}

/// Every mount point of a filesystem on the drive, NUL stripped, for `find_blockers`.
pub fn mount_points(objs: &ManagedObjects, drive: &OwnedObjectPath) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = members(objs, drive.as_str())
        .into_keys()
        .filter_map(|path| objs.0.get(path)?.get(FILESYSTEM_IFACE))
        .flat_map(mount_bytes)
        .map(|bytes| PathBuf::from(OsStr::from_bytes(&bytes)))
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Every block on the drive, by object path, with its depth: 0 for blocks whose `Drive` is
/// `drive`, one more for each `CryptoBackingDevice` hop.
fn members<'a>(objs: &'a ManagedObjects, drive: &str) -> BTreeMap<&'a str, u32> {
    let mut found: BTreeMap<&str, u32> = objs
        .0
        .iter()
        .filter(|(_, ifaces)| {
            ifaces
                .get(BLOCK_IFACE)
                .and_then(|b| object_path(b, "Drive"))
                .is_some_and(|d| d == drive)
        })
        .map(|(path, _)| (path.as_str(), 0))
        .collect();
    // Each pass adds one level of cleartext devices. A chain never has more levels than objects.
    for _ in 0..objs.0.len() {
        let next: Vec<(&str, u32)> = objs
            .0
            .iter()
            .filter(|(path, _)| !found.contains_key(path.as_str()))
            .filter_map(|(path, ifaces)| {
                let backing = object_path(ifaces.get(BLOCK_IFACE)?, "CryptoBackingDevice")?;
                let depth = found.get(backing)?;
                Some((path.as_str(), depth + 1))
            })
            .collect();
        if next.is_empty() {
            break;
        }
        found.extend(next);
    }
    found
}

/// `MountPoints` (`aay`), each without its trailing NUL. Empty entries are dropped.
fn mount_bytes(fs: &Props) -> Vec<Vec<u8>> {
    let Some(Value::Array(points)) = fs.get("MountPoints").map(|v| &**v) else {
        return Vec::new();
    };
    points
        .iter()
        .filter_map(|point| {
            let Value::Array(bytes) = point else {
                return None;
            };
            let mut out: Vec<u8> = bytes
                .iter()
                .map(|b| match b {
                    Value::U8(b) => Some(*b),
                    _ => None,
                })
                .collect::<Option<_>>()?;
            while out.last() == Some(&0) {
                out.pop();
            }
            (!out.is_empty()).then_some(out)
        })
        .collect()
}

/// Blocking. Scans `<proc>/<pid>/{cwd,root,exe,fd/*,maps}` for paths under any of `mounts`.
/// Returns the blockers sorted by pid, and whether processes of other users may also hold the
/// mounts (their `fd/` is not readable).
pub fn find_blockers(
    proc_root: &Path,
    mounts: &[PathBuf],
    max_paths_per_proc: usize,
) -> (Vec<Blocker>, bool) {
    let self_uid = rustix::process::getuid().as_raw();
    find_blockers_as(proc_root, mounts, max_paths_per_proc, self_uid)
}

/// `find_blockers` for the user `self_uid`. Tests pass another uid to act as a process of
/// another user.
fn find_blockers_as(
    proc_root: &Path,
    mounts: &[PathBuf],
    max_paths_per_proc: usize,
    self_uid: u32,
) -> (Vec<Blocker>, bool) {
    let mounts: Vec<&[u8]> = mounts
        .iter()
        .map(|m| m.as_os_str().as_bytes())
        .filter(|m| !m.is_empty())
        .collect();
    let Ok(entries) = fs::read_dir(proc_root) else {
        // Nothing could be checked.
        return (Vec::new(), true);
    };
    let boot_id = fs::read_to_string(proc_root.join("sys/kernel/random/boot_id"))
        .ok()
        .and_then(|text| BootId::from_uuid_text(&text))
        .unwrap_or(BootId(0));
    let mut others_may_hold = false;
    let mut blockers = Vec::new();
    let mut buf = Vec::new();
    let mut stat = PidStat::default();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(pid) = std::str::from_utf8(name.as_bytes())
            .ok()
            .and_then(|s| s.parse::<u32>().ok())
        else {
            continue;
        };
        let dir = entry.path();
        let scan = scan_process(&dir, &mounts, self_uid, &mut buf, &mut stat);
        others_may_hold |= scan.others;
        let Some((start_ticks, hits)) = scan.hits.filter(|(_, h)| !h.is_empty()) else {
            continue;
        };
        let mut comm = String::new();
        sanitize_text(&stat.comm_raw, COMM_MAX, &mut comm);
        let paths: Vec<Arc<str>> = hits
            .iter()
            .take(max_paths_per_proc)
            .map(|p| {
                let mut text = String::new();
                sanitize_text(p, EXE_LINK_MAX, &mut text);
                Arc::from(text)
            })
            .collect();
        blockers.push(Blocker {
            key: ProcessKey {
                start_ticks,
                boot_id,
                pid,
            },
            name: Arc::from(comm),
            app: None,
            paths: paths.into(),
        });
    }
    blockers.sort_by_key(|b| b.key.pid);
    (blockers, others_may_hold)
}

/// What one process scan found.
struct Scan {
    /// The start time and the distinct hits, sorted. `None` when the process is gone or changed
    /// during the scan.
    hits: Option<(u64, BTreeSet<Vec<u8>>)>,
    /// `fd/` of another user's process was not readable.
    others: bool,
}

fn scan_process(
    dir: &Path,
    mounts: &[&[u8]],
    self_uid: u32,
    buf: &mut Vec<u8>,
    stat: &mut PidStat,
) -> Scan {
    let mut scan = Scan {
        hits: None,
        others: false,
    };
    let Some(start) = read_start(dir, buf, stat) else {
        return scan;
    };
    let mut hits = BTreeSet::new();
    let check = |target: &[u8], hits: &mut BTreeSet<Vec<u8>>| {
        if mounts.iter().any(|m| under(target, m)) {
            hits.insert(target.to_vec());
        }
    };
    for link in ["cwd", "root", "exe"] {
        if let Ok(target) = fs::read_link(dir.join(link)) {
            check(target.as_os_str().as_bytes(), &mut hits);
        }
    }
    match fs::read_dir(dir.join("fd")) {
        Ok(fds) => {
            for fd in fds.flatten() {
                if let Ok(target) = fs::read_link(fd.path()) {
                    check(target.as_os_str().as_bytes(), &mut hits);
                }
            }
        }
        Err(e) if denied(&e) => {
            scan.others = fs::metadata(dir).is_ok_and(|m| m.uid() != self_uid);
        }
        Err(_) => {}
    }
    buf.clear();
    if let Ok(file) = fs::File::open(dir.join("maps"))
        && file.take(MAPS_MAX).read_to_end(buf).is_ok()
    {
        for line in buf.split(|&b| b == b'\n') {
            if let Some(path) = maps_path(line) {
                check(path, &mut hits);
            }
        }
    }
    // A process that exited, or a new one with the same pid, is skipped.
    let mut again = PidStat::default();
    if read_start(dir, buf, &mut again) == Some(start) {
        scan.hits = Some((start, hits));
    }
    scan
}

/// `stat` field 22, or `None` when the process is gone.
fn read_start(dir: &Path, buf: &mut Vec<u8>, stat: &mut PidStat) -> Option<u64> {
    buf.clear();
    let file = fs::File::open(dir.join("stat")).ok()?;
    file.take(STAT_MAX).read_to_end(buf).ok()?;
    parse_pid_stat(buf, stat).ok()?;
    Some(stat.start_ticks)
}

/// `EACCES` or `EPERM`.
fn denied(e: &std::io::Error) -> bool {
    e.kind() == ErrorKind::PermissionDenied
        || e.raw_os_error() == Some(rustix::io::Errno::PERM.raw_os_error())
}

/// `path` equals `mount` or lies below it. `/a/STICK2` is not under `/a/STICK`.
fn under(path: &[u8], mount: &[u8]) -> bool {
    match path.strip_prefix(mount) {
        Some([] | [b'/', ..]) => true,
        Some(_) => mount.ends_with(b"/"),
        None => false,
    }
}

/// The path column of one `maps` line: everything after the fifth field. A ` (deleted)` suffix
/// is dropped.
fn maps_path(line: &[u8]) -> Option<&[u8]> {
    let mut rest = line;
    for _ in 0..5 {
        let start = rest.iter().position(|&b| b != b' ')?;
        rest = &rest[start..];
        let end = rest.iter().position(|&b| b == b' ')?;
        rest = &rest[end..];
    }
    let start = rest.iter().position(|&b| b != b' ')?;
    let path = &rest[start..];
    Some(path.strip_suffix(b" (deleted)").unwrap_or(path))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    use std::os::unix::fs::{PermissionsExt, symlink};

    use zbus::zvariant::{ObjectPath, OwnedValue};

    use crate::workers::dbus_udisks::Interfaces;

    const DRIVE: &str = "/org/freedesktop/UDisks2/drives/SanDisk_Cruzer_1";
    const SDB: &str = "/org/freedesktop/UDisks2/block_devices/sdb";
    const SDB1: &str = "/org/freedesktop/UDisks2/block_devices/sdb1";
    const SDB2: &str = "/org/freedesktop/UDisks2/block_devices/sdb2";
    const DM3: &str = "/org/freedesktop/UDisks2/block_devices/dm_2d3";
    const SDA1: &str = "/org/freedesktop/UDisks2/block_devices/sda1";
    const STICK: &str = "/run/media/u/STICK";
    const BOOT_ID: &str = "0123456789abcdef0123456789abcdef";

    fn owned(v: Value<'static>) -> OwnedValue {
        OwnedValue::try_from(v).expect("owned value")
    }

    fn path(p: &'static str) -> Value<'static> {
        Value::from(ObjectPath::try_from(p).expect("object path"))
    }

    fn block(drive: &'static str, backing: &'static str) -> Props {
        Props::from([
            ("Drive".to_owned(), owned(path(drive))),
            ("CryptoBackingDevice".to_owned(), owned(path(backing))),
        ])
    }

    fn fs_at(mounts: &[&str]) -> Props {
        let points: Vec<Vec<u8>> = mounts
            .iter()
            .map(|m| {
                let mut b = m.as_bytes().to_vec();
                b.push(0);
                b
            })
            .collect();
        Props::from([("MountPoints".to_owned(), owned(Value::from(points)))])
    }

    fn object(ifaces: Vec<(&str, Props)>) -> Interfaces {
        ifaces
            .into_iter()
            .map(|(name, p)| (name.to_owned(), p))
            .collect()
    }

    /// A USB stick: `sdb1` vfat mounted, `sdb2` LUKS with `dm-3` ext4 mounted, plus an
    /// unrelated mounted `sda1`.
    pub(crate) fn stick_with_luks(ejectable: bool) -> ManagedObjects {
        let drive = Props::from([
            ("Ejectable".to_owned(), owned(Value::Bool(ejectable))),
            ("CanPowerOff".to_owned(), owned(Value::Bool(true))),
        ]);
        let encrypted = Props::from([("CleartextDevice".to_owned(), owned(path(DM3)))]);
        ManagedObjects(BTreeMap::from([
            (DRIVE.to_owned(), object(vec![(DRIVE_IFACE, drive)])),
            (
                SDB.to_owned(),
                object(vec![(BLOCK_IFACE, block(DRIVE, "/"))]),
            ),
            (
                SDB1.to_owned(),
                object(vec![
                    (BLOCK_IFACE, block(DRIVE, "/")),
                    (FILESYSTEM_IFACE, fs_at(&[STICK])),
                ]),
            ),
            (
                SDB2.to_owned(),
                object(vec![
                    (BLOCK_IFACE, block(DRIVE, "/")),
                    (ENCRYPTED_IFACE, encrypted),
                ]),
            ),
            (
                DM3.to_owned(),
                object(vec![
                    (BLOCK_IFACE, block("/", SDB2)),
                    (FILESYSTEM_IFACE, fs_at(&["/run/media/u/DATA"])),
                ]),
            ),
            (
                SDA1.to_owned(),
                object(vec![
                    (
                        BLOCK_IFACE,
                        block("/org/freedesktop/UDisks2/drives/Other", "/"),
                    ),
                    (FILESYSTEM_IFACE, fs_at(&["/home"])),
                ]),
            ),
        ]))
    }

    fn op(p: &str) -> OwnedObjectPath {
        OwnedObjectPath::try_from(p).expect("object path")
    }

    #[test]
    fn plan_order_luks_first() {
        let objs = stick_with_luks(true);
        let got = plan(&objs, &op(DRIVE));
        assert_eq!(got.unmount, [op(DM3), op(SDB1)]);
        assert_eq!(got.lock, [op(SDB2)]);
        assert_eq!(got.drive, op(DRIVE));
        assert!(!got.power_off);

        // Not ejectable but can power off.
        let got = plan(&stick_with_luks(false), &op(DRIVE));
        assert!(got.power_off);

        assert_eq!(
            mount_points(&objs, &op(DRIVE)),
            [PathBuf::from("/run/media/u/DATA"), PathBuf::from(STICK)]
        );
    }

    #[test]
    fn locked_luks_and_unmounted_fs_skipped() {
        let mut objs = stick_with_luks(true);
        objs.0.remove(DM3);
        let sdb2 = objs.0.get_mut(SDB2).expect("sdb2");
        sdb2.insert(
            ENCRYPTED_IFACE.to_owned(),
            Props::from([("CleartextDevice".to_owned(), owned(path("/")))]),
        );
        let sdb1 = objs.0.get_mut(SDB1).expect("sdb1");
        sdb1.insert(FILESYSTEM_IFACE.to_owned(), fs_at(&[]));
        let got = plan(&objs, &op(DRIVE));
        assert!(got.unmount.is_empty(), "{got:?}");
        assert!(got.lock.is_empty(), "{got:?}");
    }

    /// A fake `/proc` under `target/tmp`, removed on drop.
    pub(crate) struct FakeProc {
        dir: tempfile::TempDir,
    }

    impl FakeProc {
        pub(crate) fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            fs::create_dir_all(&base).expect("create target/tmp");
            let dir = tempfile::Builder::new()
                .prefix("tl-engine-eject-")
                .tempdir_in(base)
                .expect("tempdir");
            let fake = Self { dir };
            let boot = fake.path().join("sys/kernel/random");
            fs::create_dir_all(&boot).expect("boot dir");
            let uuid = format!(
                "{}-{}-{}-{}-{}",
                &BOOT_ID[..8],
                &BOOT_ID[8..12],
                &BOOT_ID[12..16],
                &BOOT_ID[16..20],
                &BOOT_ID[20..]
            );
            fs::write(boot.join("boot_id"), uuid).expect("boot id");
            fake
        }

        pub(crate) fn path(&self) -> &Path {
            self.dir.path()
        }

        /// `<pid>/stat` with `comm` and start time `1000 + pid`, and an empty `fd/`.
        pub(crate) fn process(&self, pid: u32, comm: &str) -> PathBuf {
            let dir = self.path().join(pid.to_string());
            fs::create_dir_all(dir.join("fd")).expect("fd dir");
            let mut line = format!("{pid} ({comm}) S 1 {pid} {pid} 0");
            // Fields 8 to 41; field 22 is the start time.
            for field in 8..=41 {
                if field == 22 {
                    line.push_str(&format!(" {}", 1000 + pid));
                } else {
                    line.push_str(" 0");
                }
            }
            line.push('\n');
            fs::write(dir.join("stat"), line).expect("stat");
            dir
        }

        pub(crate) fn link(&self, pid: u32, name: &str, target: &str) {
            symlink(target, self.path().join(pid.to_string()).join(name)).expect("symlink");
        }
    }

    #[test]
    fn blockers_cwd_hit() {
        let fake = FakeProc::new();
        fake.process(100, "gwenview");
        fake.link(100, "cwd", "/run/media/u/STICK/photos");
        fake.link(100, "root", "/");
        fake.process(200, "bash");
        fake.link(200, "fd/3", "/home/u/x");
        fake.link(200, "cwd", "/home/u");
        fs::create_dir_all(fake.path().join("self")).expect("self");

        let (blockers, others) = find_blockers(fake.path(), &[PathBuf::from(STICK)], 8);
        assert!(!others);
        assert_eq!(blockers.len(), 1, "{blockers:?}");
        let b = &blockers[0];
        assert_eq!(b.key.pid, 100);
        assert_eq!(b.key.start_ticks, 1100);
        assert_eq!(
            b.key.boot_id,
            BootId::from_uuid_text("01234567-89ab-cdef-0123-456789abcdef").expect("boot id")
        );
        assert_eq!(&*b.name, "gwenview");
        assert_eq!(b.app, None);
        assert_eq!(&b.paths[..], [Arc::from("/run/media/u/STICK/photos")]);
    }

    #[test]
    fn blockers_fd_and_maps_distinct_sorted_capped() {
        let fake = FakeProc::new();
        let dir = fake.process(150, "vlc");
        fake.link(150, "fd/4", "/run/media/u/STICK/b.mkv");
        fake.link(150, "fd/5", "/run/media/u/STICK/a.mkv");
        fake.link(150, "fd/6", "/run/media/u/STICK/a.mkv");
        fs::write(
            dir.join("maps"),
            "7f00-7f01 r--p 00000000 08:11 12      /run/media/u/STICK/lib\\ x.so (deleted)\n\
             7f01-7f02 r--p 00000000 00:00 0 \n\
             7f02-7f03 r-xp 00000000 08:01 34      /usr/lib/libc.so.6\n",
        )
        .expect("maps");
        let (blockers, _) = find_blockers(fake.path(), &[PathBuf::from(STICK)], 8);
        assert_eq!(blockers.len(), 1);
        assert_eq!(
            &blockers[0].paths[..],
            [
                Arc::from("/run/media/u/STICK/a.mkv"),
                Arc::from("/run/media/u/STICK/b.mkv"),
                Arc::from("/run/media/u/STICK/lib\\ x.so"),
            ]
        );
        let (blockers, _) = find_blockers(fake.path(), &[PathBuf::from(STICK)], 2);
        assert_eq!(blockers[0].paths.len(), 2);
    }

    #[test]
    fn blockers_unreadable_sets_others() {
        let fake = FakeProc::new();
        let dir = fake.process(300, "backup");
        let fd = dir.join("fd");
        fs::set_permissions(&fd, fs::Permissions::from_mode(0o000)).expect("chmod");
        let owner = fs::metadata(&dir).expect("metadata").uid();

        let (blockers, others) = find_blockers_as(
            fake.path(),
            &[PathBuf::from(STICK)],
            8,
            owner.wrapping_add(1),
        );
        // Our own unreadable `fd/` says nothing about other users.
        let (_, own) = find_blockers_as(fake.path(), &[PathBuf::from(STICK)], 8, owner);
        fs::set_permissions(&fd, fs::Permissions::from_mode(0o755)).expect("chmod back");

        assert!(blockers.is_empty());
        assert!(others);
        assert!(!own);
    }

    #[test]
    fn mount_prefix_needs_slash() {
        let m = STICK.as_bytes();
        assert!(!under(b"/run/media/u/STICK2/a", m));
        assert!(under(b"/run/media/u/STICK", m));
        assert!(under(b"/run/media/u/STICK/a", m));
        assert!(!under(b"/run/media/u", m));
        assert!(under(b"/a", b"/"));
    }

    #[test]
    fn maps_path_column() {
        assert_eq!(
            maps_path(b"55d0-55d1 r--p 00000000 08:01 1234       /usr/bin/a b"),
            Some(&b"/usr/bin/a b"[..])
        );
        assert_eq!(maps_path(b"55d0-55d1 rw-p 00000000 00:00 0"), None);
        assert_eq!(
            maps_path(b"55d0-55d1 r--p 00000000 08:01 1 /x (deleted)"),
            Some(&b"/x"[..])
        );
    }

    /// Reads the real `/proc` only.
    #[test]
    #[ignore = "reads the host /proc"]
    fn real_proc_scan() {
        let (blockers, _) = find_blockers(Path::new("/proc"), &[PathBuf::from("/")], 2);
        let me = std::process::id();
        assert!(blockers.iter().any(|b| b.key.pid == me), "{blockers:?}");
    }
}
