//! Search File Handles (PROC-096, UI-PWN-030): one search over the open handles of every
//! process, run in steps on the blocking pool.
//!
//! This wraps the `tl-collect` `HandleWalk` and its `Pattern` (E05-031): `fd/*` links, and with
//! `include_maps` the file paths of `maps` (one hit per path per process) and the `cwd`, `exe`
//! and `root` links. The pattern is case-insensitive; with `*` or `?` it is a glob over the
//! whole path, else a substring.
//!
//! Phases: `Initializing` from `new`, `Retrieving` with the percent while pids are left, then
//! `Done`. `Searching` is never published, because the walk reads and matches in one pass.
//! `cancel` ends the search as `Stopped` before the next pid. `Failed` only when the proc root
//! cannot be listed; an empty pid list ends `Done` with no hits.
//!
//! A process whose `fd/` cannot be read, for example another user's, counts in `skipped` and
//! gives no hits. Helper path (tasklens-qfx.19, PROC-096): after `use_helper` on a search the
//! user started with Search all (`privileged`), `helper_reads` hands out one `ReadProcessFiles`
//! per skipped process: `FD_LIST`, and with `include_maps` also `MAPS`, `CWD`, `EXE` and `ROOT`.
//! `helper_files` matches the reply with the walk's matcher and moves the process from `skipped`
//! to `searched`. A process the helper could not read stays in `skipped` (`helper_skipped`), and
//! `stop_helper` leaves every process not read yet in `skipped`. The search is `Done` only when
//! the walk is done and no helper read is left. While helper reads wait, `skipped` and the
//! percent leave them out. Paths are never logged (ADR-015).

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use rustc_hash::FxHashMap;
use tl_collect::SysRoot;
use tl_collect::proc::handles::{
    HandleHitInfo, HandleWalk, HitKind, PidFiles, SkippedPid, StepResult,
};
use tl_collect::proc::meta::{PasswdUserNames, ReadBufs, UserNames};
use tl_model::details::{HandleHit, HandleKind, HandlePhase, HandleSearchState, SearchId};
use tl_model::helper_proto::ProcFiles;
use tl_model::ids::ProcessKey;
use tl_model::time::BootId;

/// One handle search. `step` walks a budget of pids and returns the state to publish.
pub struct HandleSearch {
    pub id: SearchId,
    walk: HandleWalk,
    /// The user chose "Search all".
    privileged: bool,
    include_maps: bool,
    /// Set by `use_helper` for a privileged search; cleared by `stop_helper`.
    helper: bool,
    /// True once the walk has listed its last pid.
    walked: bool,
    /// Skipped processes not handed out by `helper_reads` yet.
    unsent: Vec<SkippedPid>,
    /// Skipped processes handed out by `helper_reads`, waiting for their reply.
    waiting: FxHashMap<ProcessKey, SkippedPid>,
    cancel: Arc<AtomicBool>,
    /// Folds into every hit's `ProcessKey`, as in the `Processes` worker.
    boot: BootId,
    users: PasswdUserNames,
    bufs: ReadBufs,
    /// Hits of the current step, before conversion.
    found: Vec<HandleHitInfo>,
    /// One chunk per step that found hits. Published as `Arc` pointers, never copied.
    chunks: Vec<Arc<[HandleHit]>>,
    phase: HandlePhase,
    /// The last state built, returned again while nothing changes.
    state: Arc<HandleSearchState>,
}

impl HandleSearch {
    /// Lists the pids of `sys_root`. The user names come from `<root>/etc/passwd`, where
    /// `<root>` is the parent of `sys_root.proc`. A boot id that cannot be read folds as 0; the
    /// `Processes` worker does not start without one, so no process row could match anyway.
    pub fn new(
        id: SearchId,
        pattern: &str,
        include_maps: bool,
        privileged: bool,
        sys_root: &SysRoot,
    ) -> Self {
        let walk = HandleWalk::start(sys_root, pattern, include_maps);
        // `HandleWalk` lists an unreadable root as empty, so tell the two apart here.
        let phase = if walk.progress().total == 0 && std::fs::read_dir(&sys_root.proc).is_err() {
            HandlePhase::Failed
        } else {
            HandlePhase::Initializing
        };
        let boot = std::fs::read_to_string(sys_root.proc_path("sys/kernel/random/boot_id"))
            .ok()
            .and_then(|text| BootId::from_uuid_text(&text))
            .unwrap_or(BootId(0));
        let base = sys_root.proc.parent().unwrap_or(Path::new("/"));
        let mut search = HandleSearch {
            id,
            walk,
            privileged,
            include_maps,
            helper: false,
            walked: false,
            unsent: Vec::new(),
            waiting: FxHashMap::default(),
            cancel: Arc::new(AtomicBool::new(false)),
            boot,
            users: PasswdUserNames::load(&base.join("etc/passwd")),
            bufs: ReadBufs::default(),
            found: Vec::new(),
            chunks: Vec::new(),
            phase,
            state: Arc::new(HandleSearchState {
                search: id,
                phase,
                progress_pct: 0,
                total_handles: 0,
                chunks: Arc::from([]),
                searched: 0,
                total: 0,
                skipped: 0,
            }),
        };
        search.state = search.build_state(false);
        search
    }

    /// Walks up to `budget` pids (at least 1), checking `cancel` before each, and returns the
    /// state to publish, with the hits of the helper replies given since the last step. Once
    /// `done()` it returns the last state again and reads nothing.
    pub fn step(&mut self, sys_root: &SysRoot, budget: usize) -> Arc<HandleSearchState> {
        if self.done() {
            return self.state.clone();
        }
        if self.cancel.load(Ordering::Relaxed) {
            self.phase = HandlePhase::Stopped;
        }
        for _ in 0..budget.max(1) {
            if self.walked || self.phase == HandlePhase::Stopped {
                break;
            }
            if self.cancel.load(Ordering::Relaxed) {
                self.phase = HandlePhase::Stopped;
                break;
            }
            let step = self
                .walk
                .step(sys_root, self.boot, 1, &mut self.bufs, &mut self.found);
            if step == StepResult::Done {
                self.walked = true;
            }
        }
        let skipped = self.walk.take_skipped();
        if self.helper {
            self.unsent.extend(skipped);
        }
        if self.phase == HandlePhase::Stopped {
            // Processes not read by the helper stay in `skipped`.
            self.unsent.clear();
            self.waiting.clear();
        } else {
            self.phase = if self.walked && self.unsent.is_empty() && self.waiting.is_empty() {
                HandlePhase::Done
            } else {
                HandlePhase::Retrieving
            };
        }
        let new_chunk = !self.found.is_empty();
        if new_chunk {
            let users = &mut self.users;
            let chunk: Arc<[HandleHit]> = self
                .found
                .drain(..)
                .map(|info| to_hit(info, users))
                .collect();
            self.chunks.push(chunk);
        }
        self.state = self.build_state(new_chunk);
        self.state.clone()
    }

    /// Stops the search before its next pid. Safe from any thread through `cancel_flag`.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    /// The flag `cancel` sets, for a caller that steps the search on another thread.
    pub fn cancel_flag(&self) -> Arc<AtomicBool> {
        self.cancel.clone()
    }

    /// True once the search is `Done`, `Stopped` or `Failed`.
    pub fn done(&self) -> bool {
        matches!(
            self.phase,
            HandlePhase::Done | HandlePhase::Stopped | HandlePhase::Failed
        )
    }

    /// The last state: `Initializing` (or `Failed`) before the first step.
    pub fn state(&self) -> Arc<HandleSearchState> {
        self.state.clone()
    }

    pub fn privileged(&self) -> bool {
        self.privileged
    }

    /// Lets a privileged search read the processes it skips through the helper. Does nothing
    /// for a search without Search all.
    pub fn use_helper(&mut self) {
        self.helper = self.privileged;
    }

    /// The helper reads to send: one `ReadProcessFiles` file set per process skipped since the
    /// last call. Each answer goes to `helper_files` or `helper_skipped`.
    pub fn helper_reads(&mut self) -> Vec<(ProcessKey, ProcFiles)> {
        if self.done() || !self.helper {
            self.unsent.clear();
            return Vec::new();
        }
        let files = if self.include_maps {
            ProcFiles::FD_LIST | ProcFiles::MAPS | ProcFiles::CWD | ProcFiles::EXE | ProcFiles::ROOT
        } else {
            ProcFiles::FD_LIST
        };
        let mut reads = Vec::with_capacity(self.unsent.len());
        for pid in self.unsent.drain(..) {
            reads.push((pid.key, files));
            self.waiting.insert(pid.key, pid);
        }
        reads
    }

    /// The helper read the files of `key`: matches them and counts the process as searched.
    /// The hits go out with the next `step`.
    pub fn helper_files(&mut self, key: ProcessKey, files: &PidFiles<'_>) {
        if self.done() {
            return;
        }
        if let Some(pid) = self.waiting.remove(&key) {
            self.walk
                .match_helper(&pid, files, &mut self.bufs, &mut self.found);
        }
    }

    /// The helper could not read `key`: it stays in `skipped`.
    pub fn helper_skipped(&mut self, key: ProcessKey) {
        self.waiting.remove(&key);
    }

    /// The helper is not running: no more helper reads, and every process not read yet stays
    /// in `skipped`.
    pub fn stop_helper(&mut self) {
        self.helper = false;
        self.unsent.clear();
        self.waiting.clear();
    }

    /// The state for the current counts. The chunk list is rebuilt only when a chunk was added.
    /// Processes that wait for the helper count neither as searched nor as skipped.
    fn build_state(&self, new_chunk: bool) -> Arc<HandleSearchState> {
        let p = self.walk.progress();
        let pending = u32::try_from(self.unsent.len() + self.waiting.len()).unwrap_or(u32::MAX);
        let skipped = p.skipped.saturating_sub(pending);
        let walked = u64::from(p.searched) + u64::from(skipped);
        let progress_pct = match self.phase {
            HandlePhase::Done => 100,
            _ if p.total == 0 => 0,
            // Below 100 while pids are left, since `walked < total` then.
            _ => u8::try_from((walked * 100 / u64::from(p.total)).min(100)).unwrap_or(100),
        };
        let chunks = if new_chunk {
            Arc::from(self.chunks.as_slice())
        } else {
            self.state.chunks.clone()
        };
        Arc::new(HandleSearchState {
            search: self.id,
            phase: self.phase,
            progress_pct,
            total_handles: p.handles,
            chunks,
            searched: p.searched,
            total: p.total,
            skipped,
        })
    }
}

fn to_hit(info: HandleHitInfo, users: &mut PasswdUserNames) -> HandleHit {
    HandleHit {
        key: info.key,
        name: info.comm,
        user: users.name(info.uid),
        kind: match info.kind {
            HitKind::Fd => HandleKind::Fd,
            HitKind::Mmap => HandleKind::Mmap,
            HitKind::Cwd => HandleKind::Cwd,
            HitKind::Exe => HandleKind::Exe,
            HitKind::Root => HandleKind::Root,
        },
        // The kernel caps fd numbers below `i32::MAX`.
        fd: info.fd.and_then(|fd| i32::try_from(fd).ok()),
        path: info.path,
    }
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::{PermissionsExt, symlink};
    use std::path::PathBuf;

    use tl_collect::proc::handles::Pattern;

    use super::*;

    /// A fake `/proc` and `/etc/passwd` under `target/tmp`, removed on drop.
    struct Fake {
        dir: tempfile::TempDir,
    }

    impl Fake {
        fn new() -> Self {
            let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/tmp");
            fs::create_dir_all(&base).expect("create target/tmp");
            let dir = tempfile::Builder::new()
                .prefix("tl-engine-handles-")
                .tempdir_in(base)
                .expect("tempdir");
            let fake = Fake { dir };
            fake.write(
                "proc/sys/kernel/random/boot_id",
                b"00112233-4455-6677-8899-aabbccddeeff\n",
            );
            fake.write(
                "etc/passwd",
                format!(
                    "me:x:{}:1000::/home/me:/bin/sh\n",
                    rustix::process::geteuid().as_raw()
                )
                .as_bytes(),
            );
            fake
        }

        fn path(&self, rel: &str) -> PathBuf {
            self.dir.path().join(rel)
        }

        fn root(&self) -> SysRoot {
            SysRoot::at(self.dir.path())
        }

        fn write(&self, rel: &str, bytes: &[u8]) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
            fs::write(path, bytes).expect("write");
        }

        fn link(&self, rel: &str, target: &str) {
            let path = self.path(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
            symlink(target, path).expect("symlink");
        }

        /// A process with a `stat`, an empty `fd/` and `cwd`, `exe` and `root` links.
        fn pid(&self, pid: u32, comm: &str) {
            self.write(
                &format!("proc/{pid}/stat"),
                format!(
                    "{pid} ({comm}) S 1 {pid} {pid} 0 -1 4194304 120 0 0 0 1 0 0 0 20 0 1 0 \
                     {} 5513216 230 18446744073709551615 1 1 0 0 0 0 0 0 0 0 0 0 17 2 0 0 0 0 0\n",
                    1000 + pid
                )
                .as_bytes(),
            );
            fs::create_dir_all(self.path(&format!("proc/{pid}/fd"))).expect("fd dir");
            self.link(&format!("proc/{pid}/cwd"), "/home/me");
            self.link(&format!("proc/{pid}/exe"), &format!("/usr/bin/{comm}"));
            self.link(&format!("proc/{pid}/root"), "/");
        }

        /// pid 100 holds `/var/log/x.log` as fd 3; pid 101 runs in `/srv/old.log`.
        fn two_pids() -> Self {
            let fake = Fake::new();
            fake.pid(100, "logger");
            fake.link("proc/100/fd/0", "/dev/null");
            fake.link("proc/100/fd/3", "/var/log/x.log");
            fake.pid(101, "shell");
            fake.link("proc/101/fd/1", "/dev/pts/0");
            fs::remove_file(fake.path("proc/101/cwd")).expect("rm cwd");
            fake.link("proc/101/cwd", "/srv/old.log");
            fake
        }
    }

    fn run(search: &mut HandleSearch, root: &SysRoot, budget: usize) -> Arc<HandleSearchState> {
        let mut state = search.state();
        for _ in 0..10_000 {
            if search.done() {
                break;
            }
            state = search.step(root, budget);
        }
        assert!(search.done());
        state
    }

    fn hits(state: &HandleSearchState) -> Vec<(u32, HandleKind, Option<i32>, String)> {
        let mut out: Vec<_> = state
            .chunks
            .iter()
            .flat_map(|c| c.iter())
            .map(|h| (h.key.pid, h.kind, h.fd, h.path.to_string()))
            .collect();
        out.sort_by(|a, b| (a.0, format!("{:?}", a.1)).cmp(&(b.0, format!("{:?}", b.1))));
        out
    }

    #[test]
    fn glob_and_substring_matching() {
        let mut lower = String::new();
        assert!(Pattern::new("FIRE").matches("/opt/firefox/firefox", &mut lower));
        assert!(Pattern::new("fi?e").matches("fire", &mut lower));
        assert!(Pattern::new("fi*x").matches("firefox", &mut lower));
        assert!(!Pattern::new("fi?e").matches("fiire", &mut lower));
        assert!(!Pattern::new("*.log").matches("/var/log", &mut lower));
    }

    #[test]
    fn fixture_walk_hits() {
        let fake = Fake::two_pids();
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(1), "*.log", true, false, &root);
        assert_eq!(search.state().phase, HandlePhase::Initializing);
        let state = run(&mut search, &root, 50);
        assert_eq!(
            hits(&state),
            [
                (100, HandleKind::Fd, Some(3), "/var/log/x.log".to_owned()),
                (101, HandleKind::Cwd, None, "/srv/old.log".to_owned()),
            ]
        );
        assert_eq!(state.phase, HandlePhase::Done);
        assert_eq!(state.search, SearchId(1));
        assert_eq!((state.searched, state.total, state.skipped), (2, 2, 0));
        assert_eq!(state.progress_pct, 100);
        // fd 0 and 3 of pid 100, fd 1 of pid 101.
        assert_eq!(state.total_handles, 3);

        let hit = state
            .chunks
            .iter()
            .flat_map(|c| c.iter())
            .next()
            .expect("hit");
        assert_eq!(&*hit.name, "logger");
        assert_eq!(&*hit.user, "me");
        assert_eq!(hit.key.start_ticks, 1100);
        assert_eq!(
            Some(hit.key.boot_id),
            BootId::from_uuid_text("00112233-4455-6677-8899-aabbccddeeff")
        );
    }

    #[test]
    fn maps_only_with_flag() {
        let fake = Fake::two_pids();
        fake.write(
            "proc/100/maps",
            b"5612e1a00000-5612e1a02000 r--p 00000000 08:01 262301 /opt/lib/libfoo.so\n\
              5612e1a02000-5612e1a07000 r-xp 00002000 08:01 262301 /opt/lib/libfoo.so\n",
        );
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(2), "libfoo", false, false, &root);
        let state = run(&mut search, &root, 50);
        assert!(hits(&state).is_empty(), "{:?}", hits(&state));
        assert_eq!(state.phase, HandlePhase::Done);

        // With the flag the path mapped twice gives one hit.
        let mut search = HandleSearch::new(SearchId(3), "libfoo", true, false, &root);
        let state = run(&mut search, &root, 50);
        assert_eq!(
            hits(&state),
            [(100, HandleKind::Mmap, None, "/opt/lib/libfoo.so".to_owned())]
        );
    }

    #[test]
    fn skipped_without_helper_found_with_helper() {
        // Security regression: another user's process gives no hit and counts as skipped. The
        // search has no helper here; `lab_helper.rs` has the helper half.
        if rustix::process::geteuid().is_root() {
            return;
        }
        let fake = Fake::two_pids();
        let fd_dir = fake.path("proc/101/fd");
        fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o000)).expect("chmod fd");
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(4), "*.log", true, true, &root);
        assert!(search.privileged());
        let state = run(&mut search, &root, 50);
        fs::set_permissions(&fd_dir, fs::Permissions::from_mode(0o755)).expect("restore fd");

        assert_eq!(
            hits(&state),
            [(100, HandleKind::Fd, Some(3), "/var/log/x.log".to_owned())]
        );
        assert_eq!((state.searched, state.total, state.skipped), (1, 2, 1));
        assert_eq!(state.phase, HandlePhase::Done);
        assert_eq!(state.progress_pct, 100);
    }

    #[test]
    fn cancel_stops_walk() {
        let fake = Fake::new();
        for pid in 1..=5 {
            fake.pid(pid, "p");
        }
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(5), "", true, false, &root);
        let first = search.step(&root, 1);
        assert_eq!(first.phase, HandlePhase::Retrieving);
        assert_eq!(first.searched, 1);

        search.cancel();
        let state = search.step(&root, 1);
        assert!(search.done());
        assert_eq!(state.phase, HandlePhase::Stopped);
        assert_eq!(state.searched, 1);
        // The hits found before the cancel stay.
        assert_eq!(state.chunks.len(), 1);
        // A stopped search reads nothing more.
        let again = search.step(&root, 50);
        assert!(Arc::ptr_eq(&state, &again));

        // The shared flag stops a search too.
        let mut search = HandleSearch::new(SearchId(6), "", true, false, &root);
        search.cancel_flag().store(true, Ordering::Relaxed);
        let state = search.step(&root, 50);
        assert_eq!((state.phase, state.searched), (HandlePhase::Stopped, 0));
    }

    #[test]
    fn progress_and_phase() {
        let fake = Fake::new();
        for pid in 1..=530 {
            fake.pid(pid, "p");
        }
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(7), "nothing-matches", true, false, &root);
        let init = search.state();
        assert_eq!(
            (init.phase, init.progress_pct, init.total),
            (HandlePhase::Initializing, 0, 530)
        );

        let state = search.step(&root, 10);
        assert_eq!(state.phase, HandlePhase::Retrieving);
        assert_eq!((state.searched, state.total), (10, 530));
        // 10 of 530 is 1.9 %.
        assert_eq!(state.progress_pct, 1);
        assert!(state.chunks.is_empty());
        // No hits: the chunk list is shared, not rebuilt.
        let next = search.step(&root, 10);
        assert!(Arc::ptr_eq(&state.chunks, &next.chunks));
        assert_eq!(next.progress_pct, 3);

        let state = run(&mut search, &root, 50);
        assert_eq!((state.phase, state.progress_pct), (HandlePhase::Done, 100));
        assert_eq!(state.searched, 530);
    }

    #[test]
    fn empty_and_unlisted_roots() {
        // An empty pid list ends Done with no hits.
        let fake = Fake::new();
        fs::remove_dir_all(fake.path("proc/sys")).expect("rm sys");
        let root = fake.root();
        let mut search = HandleSearch::new(SearchId(8), "x", true, false, &root);
        assert_eq!(search.state().phase, HandlePhase::Initializing);
        let state = run(&mut search, &root, 50);
        assert_eq!((state.phase, state.total), (HandlePhase::Done, 0));
        assert!(state.chunks.is_empty());

        // A proc root that cannot be listed fails at once and never steps.
        fs::remove_dir_all(fake.path("proc")).expect("rm proc");
        let mut search = HandleSearch::new(SearchId(9), "x", true, false, &root);
        assert!(search.done());
        let state = search.step(&root, 50);
        assert_eq!(state.phase, HandlePhase::Failed);
    }
}
