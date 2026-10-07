//! tasklens-watch: the low-memory watcher run by `tasklens-lowmem.service` (E32, PROC-097).
//!
//! A separate small binary so that the whole process fits in 4 MiB and `mlockall` succeeds
//! under the default 8 MiB RLIMIT_MEMLOCK (tasklens-m1f.19). `tasklens --watch-memory` execs it.

#[allow(dead_code, reason = "only tests call `MemInfoReader::read`")]
mod watch_memory;

fn main() -> std::process::ExitCode {
    watch_memory::run_watch_memory()
}
