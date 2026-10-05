//! P2 per-process columns (R-163 item 2): exe facts, session, security label, elevation,
//! sandbox, scheduling policy, OOM score, TTY, working directory, context switches and the
//! owning package (PROC-027, PROC-028, PROC-029, PROC-036).
//!
//! Read only for `Demand.detail_keys` (ADR-008 visible-rows rule). Every file is opened as
//! `"<pid>/<file>"` relative to the `/proc` dirfd (R-286). Every file is untrusted input: reads
//! are bounded and a failed read leaves its field `None`. The working directory, the exe path
//! and the security label are user data and are never logged (ADR-015).

use std::cell::Cell;
use std::fmt::Write as _;
use std::mem::MaybeUninit;
use std::sync::Arc;
use std::time::Instant;

use rustc_hash::{FxHashMap, FxHashSet};
use rustix::fd::AsFd;
use rustix::fs::{AtFlags, Mode, OFlags, RawDir, StatxFlags};
use rustix::io::Errno;
use tl_model::ids::ProcessKey;
use tl_model::process::{PackageStatus, ProcessExtrasP2};

use crate::pkg::PackageIndex;
use crate::proc::cgroup::CgroupPath;
use crate::proc::is_denied;
use crate::proc::meta::{ExeLink, RawMeta, ReadBufs};
use crate::proc::pid_stat::PidStat;
use crate::proc::status::{PidStatus, STATUS_READ_MAX};
use crate::proc::sweep::PidDir;
use crate::sanitize::{EXE_LINK_MAX, sanitize_link, sanitize_text};
use crate::sysroot::{SMALL_FILE_LIMIT, SysRoot, read_at, read_file};

/// Most `task/*/status` files summed per process.
pub const TASK_MAX: usize = 4096;
/// Most cached ELF headers. The cache is cleared when it grows past this.
const ELF_CACHE_MAX: usize = 4096;
/// Bytes of the ELF header read: up to and including `e_machine`.
const ELF_HEADER_LEN: usize = 20;
/// Read limit for `attr/current`, in bytes.
const LABEL_MAX: usize = 4096;
/// `sessionid` of a process outside any audit session.
const NO_SESSION: u32 = u32::MAX;
/// The kernel's fixed inode number of the initial user namespace (`PROC_USER_INIT_INO`). Used
/// when PID 1's `ns/user` cannot be read, which is the normal case without root.
const PROC_USER_INIT_INO: u64 = 0xEFFF_FFFD;
/// LSMs that set a process label in `attr/current`.
const MAJOR_LSMS: [&str; 4] = ["apparmor", "selinux", "smack", "tomoyo"];
/// getdents buffer for one `task/` listing.
const DIRENT_BUF: usize = 16 * 1024;

thread_local! {
    static ELF_READS: Cell<u32> = const { Cell::new(0) };
}

/// Test hook: how many ELF headers were read on this thread.
#[doc(hidden)]
pub fn elf_reads() -> u32 {
    ELF_READS.with(Cell::get)
}

/// Labels built once, so a read does not allocate them.
struct Labels {
    flatpak: Arc<str>,
    snap: Arc<str>,
    bwrap: Arc<str>,
    seccomp_nnp: Arc<str>,
    seccomp: Arc<str>,
    none: Arc<str>,
    sched: [Arc<str>; 6],
}

impl Labels {
    fn new() -> Self {
        Self {
            flatpak: Arc::from("Flatpak"),
            snap: Arc::from("Snap"),
            bwrap: Arc::from("bwrap"),
            seccomp_nnp: Arc::from("seccomp + NoNewPrivs"),
            seccomp: Arc::from("seccomp"),
            none: Arc::from("None"),
            sched: ["Normal", "FIFO", "RR", "Batch", "Idle", "Deadline"].map(Arc::from),
        }
    }

    /// Stat field 41 as text. Unknown policies give `None`.
    fn sched(&self, policy: u32) -> Option<Arc<str>> {
        let i = match policy {
            0 => 0,
            1 => 1,
            2 => 2,
            3 => 3,
            5 => 4,
            6 => 5,
            _ => return None,
        };
        Some(self.sched[i].clone())
    }
}

/// Fills `ProcessExtrasP2` for the visible rows and keeps the state that spans sweeps.
pub struct ExtrasP2Reader {
    /// (dev, inode) of an exe to its ELF (class, machine). `None` for a file that is not ELF.
    elf: FxHashMap<(u64, u64), Option<(u8, u16)>>,
    /// The last context switch sum of each key.
    prev_ctx: FxHashMap<ProcessKey, u64>,
    /// The inode of the initial user namespace, read once.
    init_userns: Option<u64>,
    /// True when a major LSM is active, read once from `/sys/kernel/security/lsm`.
    lsm_active: Option<bool>,
    labels: Labels,
    path: String,
    dirents: Vec<MaybeUninit<u8>>,
}

impl Default for ExtrasP2Reader {
    fn default() -> Self {
        Self::new()
    }
}

impl std::fmt::Debug for ExtrasP2Reader {
    // Counts only (ADR-015).
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ExtrasP2Reader")
            .field("elf", &self.elf.len())
            .field("prev_ctx", &self.prev_ctx.len())
            .finish_non_exhaustive()
    }
}

impl ExtrasP2Reader {
    pub fn new() -> Self {
        Self {
            elf: FxHashMap::default(),
            prev_ctx: FxHashMap::default(),
            init_userns: None,
            lsm_active: None,
            labels: Labels::new(),
            path: String::new(),
            dirents: Vec::new(),
        }
    }

    /// Fills one entry for `key`. `stat`, `status` and `meta` come from the sweep (E05-A1);
    /// `status` is `None` when its last read failed. `pkg` is the E05-034 index. Never panics; a
    /// failed read leaves its field `None`.
    #[allow(clippy::too_many_arguments, reason = "sweep state passed by reference")]
    pub fn read(
        &mut self,
        root: &SysRoot,
        dir: PidDir<'_>,
        key: ProcessKey,
        stat: &PidStat,
        status: Option<&PidStatus>,
        meta: &RawMeta,
        pkg: &mut PackageIndex,
        now: Instant,
        bufs: &mut ReadBufs,
    ) -> ProcessExtrasP2 {
        let mut out = ProcessExtrasP2 {
            key: Some(key),
            ..ProcessExtrasP2::default()
        };

        self.read_exe(dir, &mut out, &mut bufs.small);
        out.session_id = self
            .read_number::<u32>(dir, "sessionid", &mut bufs.small)
            .filter(|&s| s != NO_SESSION);
        out.security_label = self.read_label(root, dir, bufs);
        if let Some(s) = status {
            out.elevated = Some(elevated(s));
        }
        out.sandbox = self.sandbox(dir, status, meta.cgroup.as_ref());
        out.sched_policy_text = self.labels.sched(stat.policy);
        out.oom_score = self.read_number::<i32>(dir, "oom_score", &mut bufs.small);
        out.oom_score_adj = self.read_number::<i16>(dir, "oom_score_adj", &mut bufs.small);
        out.tty = decode_tty(stat.tty_nr).map(Arc::from);
        out.cwd = self.read_cwd(dir, &mut bufs.text);

        if let Some((vol, invol)) = self.ctx_switches(dir, &mut bufs.small) {
            out.ctx_switches_voluntary = Some(vol);
            out.ctx_switches_involuntary = Some(invol);
            let total = vol.saturating_add(invol);
            // A sum that went down (a thread exited) gives no delta, as for any counter reset.
            out.ctx_switches_delta = self
                .prev_ctx
                .insert(key, total)
                .and_then(|old| total.checked_sub(old));
        } else {
            self.prev_ctx.remove(&key);
        }

        self.package(meta, &mut out, pkg, now);
        out
    }

    /// Drops cached state of dead keys.
    pub fn retain_live(&mut self, live: &FxHashSet<ProcessKey>) {
        self.prev_ctx.retain(|k, _| live.contains(k));
    }

    /// Sets `path` to `"<pid>/<file>"`.
    fn pid_path(&mut self, pid: u32, file: &str) -> &str {
        self.path.clear();
        let _ = write!(self.path, "{pid}/{file}");
        &self.path
    }

    /// `exe_mtime_s`, `exe_size_bytes` from `statx` of the exe through an `O_PATH` fd, and
    /// `elf_class`, `elf_machine` from its first 20 bytes, cached by (dev, inode).
    fn read_exe(&mut self, dir: PidDir<'_>, out: &mut ProcessExtrasP2, buf: &mut Vec<u8>) {
        let proc_fd = dir.proc_fd;
        let name = self.pid_path(dir.pid, "exe");
        let Ok(fd) =
            rustix::fs::openat(proc_fd, name, OFlags::PATH | OFlags::CLOEXEC, Mode::empty())
        else {
            return;
        };
        let Ok(st) = rustix::fs::statx(
            fd.as_fd(),
            "",
            AtFlags::EMPTY_PATH,
            StatxFlags::MTIME | StatxFlags::SIZE | StatxFlags::INO,
        ) else {
            return;
        };
        out.exe_mtime_s = Some(st.stx_mtime.tv_sec);
        out.exe_size_bytes = Some(st.stx_size);

        let dev = (u64::from(st.stx_dev_major) << 32) | u64::from(st.stx_dev_minor);
        let id = (dev, st.stx_ino);
        let elf = match self.elf.get(&id) {
            Some(&cached) => cached,
            None => {
                // An O_PATH fd cannot be read, so the header comes from a second open.
                ELF_READS.with(|c| c.set(c.get() + 1));
                let name = self.pid_path(dir.pid, "exe");
                if read_at(proc_fd, name, buf, ELF_HEADER_LEN).is_err() {
                    return;
                }
                let elf = parse_elf_header(buf);
                if self.elf.len() >= ELF_CACHE_MAX {
                    self.elf.clear();
                }
                self.elf.insert(id, elf);
                elf
            }
        };
        if let Some((class, machine)) = elf {
            out.elf_class = (class != 0).then_some(class);
            out.elf_machine = Some(machine);
        }
    }

    /// One trimmed decimal number from a small `/proc/<pid>` file.
    fn read_number<T: std::str::FromStr>(
        &mut self,
        dir: PidDir<'_>,
        file: &str,
        buf: &mut Vec<u8>,
    ) -> Option<T> {
        let name = self.pid_path(dir.pid, file);
        read_at(dir.proc_fd, name, buf, SMALL_FILE_LIMIT).ok()?;
        std::str::from_utf8(buf.trim_ascii()).ok()?.parse().ok()
    }

    /// `attr/current` without its trailing NUL and newline, sanitized. `None` when no major LSM
    /// is active.
    fn read_label(
        &mut self,
        root: &SysRoot,
        dir: PidDir<'_>,
        bufs: &mut ReadBufs,
    ) -> Option<Arc<str>> {
        let active = *self.lsm_active.get_or_insert_with(|| {
            read_file(
                &root.sys_path("kernel/security/lsm"),
                &mut bufs.small,
                SMALL_FILE_LIMIT,
            )
            .is_ok_and(|_| lsm_list_has_major(&bufs.small))
        });
        if !active {
            return None;
        }
        let name = self.pid_path(dir.pid, "attr/current");
        read_at(dir.proc_fd, name, &mut bufs.small, LABEL_MAX).ok()?;
        let mut raw = bufs.small.as_slice();
        while let Some(rest) = raw.strip_suffix(b"\0").or_else(|| raw.strip_suffix(b"\n")) {
            raw = rest;
        }
        sanitize_text(raw, LABEL_MAX, &mut bufs.text);
        (!bufs.text.is_empty()).then(|| Arc::from(bufs.text.as_str()))
    }

    /// The PROC-028 sandbox rule, first match. `None` when the user namespace cannot be read
    /// and neither the Flatpak nor the Snap rule matched.
    fn sandbox(
        &mut self,
        dir: PidDir<'_>,
        status: Option<&PidStatus>,
        cgroup: Option<&CgroupPath>,
    ) -> Option<Arc<str>> {
        let rel = cgroup.map_or("", |c| &*c.rel);
        if rel.contains("app-flatpak-") {
            return Some(self.labels.flatpak.clone());
        }
        // Needs ptrace access; on any error only the cgroup rule counts.
        let name = self.pid_path(dir.pid, "root/.flatpak-info");
        if rustix::fs::statat(dir.proc_fd, name, AtFlags::empty()).is_ok() {
            return Some(self.labels.flatpak.clone());
        }
        if rel.contains("snap.") {
            return Some(self.labels.snap.clone());
        }

        let init = match self.init_userns {
            Some(ino) => ino,
            None => {
                let ino = self.userns(dir, 1).unwrap_or(PROC_USER_INIT_INO);
                self.init_userns = Some(ino);
                ino
            }
        };
        let own = match self.userns(dir, dir.pid) {
            Ok(ino) => ino,
            Err(e) => {
                if is_denied(e) {
                    tracing::trace!(pid = dir.pid, "ns/user restricted, sandbox unknown");
                }
                return None;
            }
        };
        if own != init {
            return Some(self.labels.bwrap.clone());
        }

        let s = status?;
        Some(match (s.seccomp, s.no_new_privs) {
            (Some(2), Some(true)) => self.labels.seccomp_nnp.clone(),
            (Some(2), _) => self.labels.seccomp.clone(),
            _ => self.labels.none.clone(),
        })
    }

    /// The inode in the `ns/user` link of `pid` (`user:[<inode>]`).
    fn userns(&mut self, dir: PidDir<'_>, pid: u32) -> Result<u64, Errno> {
        let name = self.pid_path(pid, "ns/user");
        let mut buf = [0u8; 64];
        let n = rustix::fs::readlinkat_raw(dir.proc_fd, name, &mut buf[..])?;
        parse_ns_link(&buf[..n.min(buf.len())]).ok_or(Errno::INVAL)
    }

    /// `readlinkat(<pid>/cwd)`, sanitized, at most 4,096 bytes.
    fn read_cwd(&mut self, dir: PidDir<'_>, text: &mut String) -> Option<Arc<str>> {
        let name = self.pid_path(dir.pid, "cwd");
        let mut buf = [0u8; EXE_LINK_MAX + 1];
        let n = rustix::fs::readlinkat_raw(dir.proc_fd, name, &mut buf[..]).ok()?;
        sanitize_link(&buf[..n.min(buf.len())], text);
        (!text.is_empty()).then(|| Arc::from(text.as_str()))
    }

    /// Sums of `voluntary_ctxt_switches` and `nonvoluntary_ctxt_switches` over at most
    /// [`TASK_MAX`] `task/*/status` files. `None` when no task file gave both values.
    fn ctx_switches(&mut self, dir: PidDir<'_>, buf: &mut Vec<u8>) -> Option<(u64, u64)> {
        let name = self.pid_path(dir.pid, "task");
        let task = rustix::fs::openat(
            dir.proc_fd,
            name,
            OFlags::RDONLY | OFlags::DIRECTORY | OFlags::CLOEXEC,
            Mode::empty(),
        )
        .ok()?;
        if self.dirents.is_empty() {
            self.dirents = vec![MaybeUninit::uninit(); DIRENT_BUF];
        }
        let mut sums: Option<(u64, u64)> = None;
        let mut tasks = 0;
        let mut entries = RawDir::new(task.as_fd(), &mut self.dirents[..]);
        while let Some(entry) = entries.next() {
            let Ok(entry) = entry else { break };
            let tid = entry.file_name().to_bytes();
            if tid.is_empty() || !tid.iter().all(u8::is_ascii_digit) {
                continue;
            }
            if tasks == TASK_MAX {
                break;
            }
            tasks += 1;
            self.path.clear();
            self.path.push_str(std::str::from_utf8(tid).unwrap_or("0"));
            self.path.push_str("/status");
            if read_at(task.as_fd(), &self.path, buf, STATUS_READ_MAX).is_err() {
                continue;
            }
            if let Some((v, n)) = parse_ctxt(buf) {
                let s = sums.get_or_insert((0, 0));
                s.0 = s.0.saturating_add(v);
                s.1 = s.1.saturating_add(n);
            }
        }
        sums
    }

    /// `package` and `package_status` (PROC-027, first match). Only an exe that no earlier rule
    /// placed is looked up in the index; a miss queues it and leaves both `None`.
    fn package(
        &self,
        meta: &RawMeta,
        out: &mut ProcessExtrasP2,
        pkg: &mut PackageIndex,
        now: Instant,
    ) {
        let ExeLink::Path { path, deleted } = &meta.exe else {
            return;
        };
        let status = if *deleted {
            Some(PackageStatus::DeletedUpgraded)
        } else if out
            .sandbox
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &self.labels.flatpak))
        {
            Some(PackageStatus::Flatpak)
        } else if out
            .sandbox
            .as_ref()
            .is_some_and(|s| Arc::ptr_eq(s, &self.labels.snap))
        {
            Some(PackageStatus::Snap)
        } else if path.starts_with("/tmp/.mount_") {
            Some(PackageStatus::AppImage)
        } else if is_wine_exe(path, meta.argv.first().map(|a| &**a)) {
            Some(PackageStatus::Wine)
        } else {
            None
        };
        if status.is_some() {
            out.package_status = status;
            return;
        }
        let Some(mtime_s) = out.exe_mtime_s else {
            return;
        };
        match pkg.get(path, mtime_s) {
            Some(Some(info)) => {
                out.package = Some(info);
                out.package_status = Some(PackageStatus::Packaged);
            }
            Some(None) => out.package_status = Some(PackageStatus::NotPackaged),
            None => pkg.request(path, mtime_s, now),
        }
    }
}

/// PROC-028 elevation, first match: euid 0; euid differs from ruid; non-zero `CapEff`.
pub fn elevated(s: &PidStatus) -> bool {
    let (ruid, euid) = (s.uid[0], s.uid[1]);
    euid == 0 || euid != ruid || s.cap_eff != 0
}

/// Decodes stat field 7 `tty_nr`: `pts/<n>` for majors 136 to 143, `tty<minor>` for major 4 with
/// minor below 64, `None` for 0, else `<major>:<minor>`.
pub fn decode_tty(tty_nr: i32) -> Option<String> {
    if tty_nr == 0 {
        return None;
    }
    let dev = tty_nr as u32;
    let major = (dev >> 8) & 0xfff;
    let minor = (dev & 0xff) | ((dev >> 12) & 0xfff00);
    Some(match major {
        136..=143 => format!("pts/{}", minor + (major - 136) * 256),
        4 if minor < 64 => format!("tty{minor}"),
        _ => format!("{major}:{minor}"),
    })
}

/// ELF (class, machine) from the first 20 bytes. Class is 32, 64 or 0 for an unknown
/// `EI_CLASS`. `None` when the magic is wrong or the header is short.
pub fn parse_elf_header(b: &[u8]) -> Option<(u8, u16)> {
    if b.len() < ELF_HEADER_LEN || &b[..4] != b"\x7fELF" {
        return None;
    }
    let class = match b[4] {
        1 => 32,
        2 => 64,
        _ => 0,
    };
    Some((class, u16::from_le_bytes([b[18], b[19]])))
}

/// True when the comma-separated LSM list names a major LSM.
fn lsm_list_has_major(list: &[u8]) -> bool {
    list.trim_ascii()
        .split(|&b| b == b',')
        .any(|name| MAJOR_LSMS.iter().any(|m| m.as_bytes() == name.trim_ascii()))
}

/// The inode of a `user:[<inode>]` link.
fn parse_ns_link(link: &[u8]) -> Option<u64> {
    let inner = link.strip_prefix(b"user:[")?.strip_suffix(b"]")?;
    std::str::from_utf8(inner).ok()?.parse().ok()
}

/// (voluntary, nonvoluntary) context switches of one `status` file.
fn parse_ctxt(buf: &[u8]) -> Option<(u64, u64)> {
    let (mut vol, mut invol) = (None, None);
    for line in buf.split(|&b| b == b'\n') {
        let Some(colon) = line.iter().position(|&b| b == b':') else {
            continue;
        };
        let slot = match &line[..colon] {
            b"voluntary_ctxt_switches" => &mut vol,
            b"nonvoluntary_ctxt_switches" => &mut invol,
            _ => continue,
        };
        *slot = std::str::from_utf8(line[colon + 1..].trim_ascii())
            .ok()
            .and_then(|v| v.parse::<u64>().ok());
    }
    Some((vol?, invol?))
}

/// The exe basename starts with `wine` and argv[0] ends in `.exe` (any case).
fn is_wine_exe(exe: &str, argv0: Option<&str>) -> bool {
    let base = exe.rsplit('/').next().unwrap_or(exe);
    base.starts_with("wine")
        && argv0.is_some_and(|a| {
            a.len() >= 4
                && a.is_char_boundary(a.len() - 4)
                && a[a.len() - 4..].eq_ignore_ascii_case(".exe")
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tty_decoding() {
        assert_eq!(decode_tty(0), None);
        assert_eq!(decode_tty(34817).as_deref(), Some("pts/1"));
        assert_eq!(decode_tty((137 << 8) | 5).as_deref(), Some("pts/261"));
        assert_eq!(decode_tty((4 << 8) | 2).as_deref(), Some("tty2"));
        assert_eq!(decode_tty((4 << 8) | 64).as_deref(), Some("4:64"));
        assert_eq!(decode_tty((5 << 8) | 1).as_deref(), Some("5:1"));
        assert!(decode_tty(-1).is_some());
    }

    #[test]
    fn elf_header() {
        let mut h = [0u8; 20];
        h[..4].copy_from_slice(b"\x7fELF");
        h[4] = 2;
        h[18] = 62;
        assert_eq!(parse_elf_header(&h), Some((64, 62)));
        h[4] = 1;
        h[18] = 3;
        assert_eq!(parse_elf_header(&h), Some((32, 3)));
        assert_eq!(parse_elf_header(&h[..19]), None);
        h[0] = 0;
        assert_eq!(parse_elf_header(&h), None);
    }

    #[test]
    fn lsm_and_ns_parsing() {
        assert!(!lsm_list_has_major(
            b"capability,landlock,lockdown,yama,bpf"
        ));
        assert!(lsm_list_has_major(b"capability,apparmor,bpf\n"));
        assert_eq!(
            parse_ns_link(b"user:[4026531837]"),
            Some(PROC_USER_INIT_INO)
        );
        assert_eq!(parse_ns_link(b"net:[4026531837]"), None);
    }

    #[test]
    fn elevation_rule() {
        let mut s = PidStatus {
            uid: [1000; 4],
            ..PidStatus::default()
        };
        assert!(!elevated(&s));
        s.cap_eff = 1 << 12;
        assert!(elevated(&s));
        s.cap_eff = 0;
        s.uid[1] = 0;
        assert!(elevated(&s));
        s.uid[1] = 1001;
        assert!(elevated(&s));
    }

    #[test]
    fn wine_rule() {
        assert!(is_wine_exe(
            "/usr/bin/wine64-preloader",
            Some("C:\\x\\Game.EXE")
        ));
        assert!(!is_wine_exe("/usr/bin/wine64", Some("winecfg")));
        assert!(!is_wine_exe("/usr/bin/foo", Some("a.exe")));
        assert!(!is_wine_exe("/usr/bin/wine", Some("é")));
    }
}
