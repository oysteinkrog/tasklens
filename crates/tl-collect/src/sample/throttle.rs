//! CPU thermal throttling (ADR-016 "CPU throttling", UI-PRF-054): Intel's
//! `/sys/devices/system/cpu/cpu<n>/thermal_throttle/{core,package}_throttle_count`. The `Cpu`
//! worker calls it once per second.
//!
//! The counters only rise. The reading is true when their sum rose since the previous read.
//! The files are listed once, when the sampler is made. A kernel without them (AMD) gives
//! `Unsupported`.

use std::fs;
use std::path::PathBuf;

use tl_model::validity::Validity;

use crate::SysRoot;
use crate::counters::Measured;
use crate::sysroot::read_u64;

/// Reads every `*_throttle_count` file.
pub struct ThrottleSampler {
    files: Vec<PathBuf>,
    buf: Vec<u8>,
    /// The sum of the previous read, `None` before the first read and after a failed one.
    prev: Option<u64>,
}

impl ThrottleSampler {
    pub fn new(root: &SysRoot) -> Self {
        let mut files = Vec::new();
        if let Ok(dir) = fs::read_dir(root.sys_path("devices/system/cpu")) {
            for entry in dir.flatten() {
                let is_cpu = entry
                    .file_name()
                    .to_str()
                    .and_then(|n| n.strip_prefix("cpu"))
                    .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()));
                if !is_cpu {
                    continue;
                }
                for name in ["core_throttle_count", "package_throttle_count"] {
                    let path = entry.path().join("thermal_throttle").join(name);
                    if path.exists() {
                        files.push(path);
                    }
                }
            }
        }
        files.sort_unstable();
        Self {
            files,
            buf: Vec::new(),
            prev: None,
        }
    }

    /// True when a count rose since the previous call. The first call is `WarmingUp`.
    pub fn sample(&mut self) -> Measured<bool> {
        if self.files.is_empty() {
            return Measured::invalid(Validity::Unsupported);
        }
        let mut sum = 0u64;
        for path in &self.files {
            match read_u64(path, &mut self.buf) {
                Ok(n) => sum = sum.saturating_add(n),
                Err(err) => {
                    self.prev = None;
                    return Measured::invalid(err.validity());
                }
            }
        }
        let prev = self.prev.replace(sum);
        match prev {
            Some(prev) => Measured::valid(sum > prev),
            None => Measured::invalid(Validity::WarmingUp),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(dir: &std::path::Path) -> SysRoot {
        SysRoot {
            proc: dir.join("proc"),
            sys: dir.join("sys"),
            run: dir.join("run"),
        }
    }

    fn write(dir: &std::path::Path, cpu: u32, name: &str, n: u64) {
        let d = dir.join(format!("sys/devices/system/cpu/cpu{cpu}/thermal_throttle"));
        fs::create_dir_all(&d).unwrap();
        fs::write(d.join(name), format!("{n}\n")).unwrap();
    }

    #[test]
    fn no_files_is_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("sys/devices/system/cpu/cpu0")).unwrap();
        let mut s = ThrottleSampler::new(&root(dir.path()));
        assert_eq!(s.sample().validity, Validity::Unsupported);
    }

    #[test]
    fn rising_count_is_throttling_once() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 0, "core_throttle_count", 3);
        write(dir.path(), 0, "package_throttle_count", 1);
        write(dir.path(), 1, "core_throttle_count", 0);
        let mut s = ThrottleSampler::new(&root(dir.path()));
        assert_eq!(s.sample().validity, Validity::WarmingUp);
        assert_eq!(s.sample(), Measured::valid(false));
        write(dir.path(), 1, "core_throttle_count", 2);
        assert_eq!(s.sample(), Measured::valid(true));
        assert_eq!(s.sample(), Measured::valid(false));
    }

    #[test]
    fn unreadable_count_restarts_the_baseline() {
        let dir = tempfile::tempdir().unwrap();
        write(dir.path(), 0, "core_throttle_count", 3);
        let mut s = ThrottleSampler::new(&root(dir.path()));
        s.sample();
        fs::write(
            dir.path()
                .join("sys/devices/system/cpu/cpu0/thermal_throttle/core_throttle_count"),
            "x\n",
        )
        .unwrap();
        assert!(!s.sample().validity.is_valid());
        write(dir.path(), 0, "core_throttle_count", 9);
        assert_eq!(s.sample().validity, Validity::WarmingUp);
    }
}
