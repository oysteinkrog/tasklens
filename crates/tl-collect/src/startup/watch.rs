//! inotify watch on the startup directories while the Startup Apps tab is visible (SYS-019, R-082).
//!
//! The `Startup` worker (E14) waits on [`StartupWatch::as_fd`] and rescans when [`StartupWatch::drain`]
//! returns true. Unit files are covered by `UnitFilesChanged` (E14), not by this watch.

use std::path::PathBuf;

use rustix::fd::{AsFd, BorrowedFd, OwnedFd};
use rustix::fs::inotify::{self, CreateFlags, ReadFlags, WatchFlags};
use rustix::io::Errno;

use super::autostart::StartupEnv;
use crate::error::CollectError;

/// Size of the reused read buffer.
const BUF_LEN: usize = 4096;
/// Size of `struct inotify_event` without the name.
const EVENT_HEADER: usize = 16;

/// A non-blocking inotify fd with one watch per startup directory. Dropping it closes the fd,
/// which removes every watch.
pub struct StartupWatch {
    fd: OwnedFd,
    /// Each directory and its watch descriptor, `None` while it is not watched.
    watched: Vec<(PathBuf, Option<i32>)>,
    buf: Vec<u8>,
}

impl StartupWatch {
    /// inotify_init1(IN_NONBLOCK | IN_CLOEXEC) and a watch on each existing directory of `dirs(env)`.
    pub fn new(env: &StartupEnv) -> Result<Self, CollectError> {
        let fd = inotify::init(CreateFlags::NONBLOCK | CreateFlags::CLOEXEC)?;
        let mut watch = Self {
            fd,
            watched: dirs(env).into_iter().map(|dir| (dir, None)).collect(),
            buf: vec![0; BUF_LEN],
        };
        watch.rewatch();
        Ok(watch)
    }

    /// Retries watches for directories that did not exist before. Call after every rescan.
    pub fn rewatch(&mut self) {
        for (dir, wd) in &mut self.watched {
            if wd.is_none() {
                match inotify::add_watch(&self.fd, dir.as_path(), watch_mask()) {
                    Ok(new_wd) => *wd = Some(new_wd),
                    Err(errno) => tracing::trace!(?dir, %errno, "startup watch not added"),
                }
            }
        }
    }

    /// Reads all pending events without blocking. True if any event arrived since the last call.
    pub fn drain(&mut self) -> Result<bool, CollectError> {
        let mut any = false;
        loop {
            let len = match rustix::io::read(&self.fd, &mut self.buf[..]) {
                Ok(len) => len,
                Err(Errno::AGAIN) => return Ok(any),
                Err(Errno::INTR) => continue,
                Err(errno) => return Err(errno.into()),
            };
            if len == 0 {
                return Ok(any);
            }
            any = true;
            let mut at = 0;
            while at + EVENT_HEADER <= len {
                let header = &self.buf[at..at + EVENT_HEADER];
                let wd = i32::from_ne_bytes([header[0], header[1], header[2], header[3]]);
                let mask = ReadFlags::from_bits_retain(u32::from_ne_bytes([
                    header[4], header[5], header[6], header[7],
                ]));
                let name_len =
                    u32::from_ne_bytes([header[12], header[13], header[14], header[15]]) as usize;
                if mask.intersects(ReadFlags::DELETE_SELF | ReadFlags::IGNORED) {
                    for (_, slot) in &mut self.watched {
                        if *slot == Some(wd) {
                            *slot = None;
                        }
                    }
                }
                at += EVENT_HEADER + name_len;
            }
        }
    }

    /// For the worker to wait on readability.
    pub fn as_fd(&self) -> BorrowedFd<'_> {
        self.fd.as_fd()
    }
}

fn watch_mask() -> WatchFlags {
    WatchFlags::CREATE
        | WatchFlags::DELETE
        | WatchFlags::MODIFY
        | WatchFlags::CLOSE_WRITE
        | WatchFlags::MOVED_FROM
        | WatchFlags::MOVED_TO
        | WatchFlags::ATTRIB
        | WatchFlags::DELETE_SELF
        | WatchFlags::MOVE_SELF
}

/// config_home/autostart, each config_dirs[i]/autostart, config_home/plasma-workspace/env,
/// config_home/plasma-workspace/shutdown. Unit files are covered by UnitFilesChanged (E14), not inotify.
/// An empty or relative `config_home` (no usable HOME) contributes no directories.
pub fn dirs(env: &StartupEnv) -> Vec<PathBuf> {
    let home = env
        .config_home
        .is_absolute()
        .then_some(env.config_home.as_path());
    let mut out = Vec::with_capacity(env.config_dirs.len() + 3);
    if let Some(home) = home {
        out.push(home.join("autostart"));
    }
    out.extend(env.config_dirs.iter().map(|dir| dir.join("autostart")));
    if let Some(home) = home {
        out.push(home.join("plasma-workspace/env"));
        out.push(home.join("plasma-workspace/shutdown"));
    }
    out
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use tempfile::TempDir;

    use super::*;

    fn env_in(tmp: &TempDir) -> StartupEnv {
        StartupEnv {
            config_home: tmp.path().join("home"),
            config_dirs: vec![tmp.path().join("xdg")],
            ..StartupEnv::default()
        }
    }

    #[test]
    fn create_triggers_once() {
        let tmp = TempDir::new().unwrap();
        let env = env_in(&tmp);
        fs::create_dir_all(env.config_home.join("autostart")).unwrap();
        let mut watch = StartupWatch::new(&env).unwrap();
        fs::write(
            env.config_home.join("autostart/foo.desktop"),
            "[Desktop Entry]\n",
        )
        .unwrap();
        assert!(watch.drain().unwrap());
        assert!(!watch.drain().unwrap());
    }

    #[test]
    fn chmod_triggers() {
        let tmp = TempDir::new().unwrap();
        let env = env_in(&tmp);
        let env_dir = env.config_home.join("plasma-workspace/env");
        fs::create_dir_all(&env_dir).unwrap();
        let script = env_dir.join("env.sh");
        fs::write(&script, "#!/bin/sh\n").unwrap();
        let mut watch = StartupWatch::new(&env).unwrap();
        assert!(!watch.drain().unwrap());
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(watch.drain().unwrap());
    }

    #[test]
    fn rewatch_new_dir() {
        let tmp = TempDir::new().unwrap();
        let env = env_in(&tmp);
        let mut watch = StartupWatch::new(&env).unwrap();
        let env_dir = env.config_home.join("plasma-workspace/env");
        fs::create_dir_all(&env_dir).unwrap();
        assert!(!watch.drain().unwrap());
        watch.rewatch();
        fs::write(env_dir.join("env.sh"), "#!/bin/sh\n").unwrap();
        assert!(watch.drain().unwrap());
    }

    #[test]
    fn idle_drain_false() {
        let tmp = TempDir::new().unwrap();
        let env = env_in(&tmp);
        fs::create_dir_all(env.config_home.join("autostart")).unwrap();
        fs::create_dir_all(tmp.path().join("xdg/autostart")).unwrap();
        let mut watch = StartupWatch::new(&env).unwrap();
        assert!(!watch.drain().unwrap());
        assert!(!watch.drain().unwrap());
    }

    #[test]
    fn deleted_dir_is_rewatched() {
        let tmp = TempDir::new().unwrap();
        let env = env_in(&tmp);
        let dir = env.config_home.join("autostart");
        fs::create_dir_all(&dir).unwrap();
        let mut watch = StartupWatch::new(&env).unwrap();
        fs::remove_dir(&dir).unwrap();
        assert!(watch.drain().unwrap());
        assert_eq!(watch.watched[0].1, None);
        fs::create_dir_all(&dir).unwrap();
        watch.rewatch();
        assert!(watch.watched[0].1.is_some());
        fs::write(dir.join("foo.desktop"), "").unwrap();
        assert!(watch.drain().unwrap());
    }

    #[test]
    fn dirs_order_and_empty_home() {
        let env = StartupEnv {
            config_home: PathBuf::from("/h"),
            config_dirs: vec![PathBuf::from("/a"), PathBuf::from("/b")],
            ..StartupEnv::default()
        };
        let want: Vec<PathBuf> = [
            "/h/autostart",
            "/a/autostart",
            "/b/autostart",
            "/h/plasma-workspace/env",
            "/h/plasma-workspace/shutdown",
        ]
        .into_iter()
        .map(PathBuf::from)
        .collect();
        assert_eq!(dirs(&env), want);
        let no_home = StartupEnv {
            config_dirs: vec![PathBuf::from("/a")],
            ..StartupEnv::default()
        };
        assert_eq!(dirs(&no_home), vec![PathBuf::from("/a/autostart")]);
    }
}
