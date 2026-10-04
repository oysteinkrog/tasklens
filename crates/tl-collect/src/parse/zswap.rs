//! zswap parameters under /sys/module/zswap/parameters (PERF-044, U-20).
//!
//! The pool size is meminfo `Zswap`; debugfs is not read.
use std::io::ErrorKind;
use std::path::Path;
use std::sync::Arc;

use crate::sysroot::read_trimmed;
use crate::{CollectError, SysRoot};

const ENABLED_ERROR: CollectError = CollectError::Parse {
    file: "zswap enabled",
    what: "value",
};
const MAX_POOL_ERROR: CollectError = CollectError::Parse {
    file: "zswap max_pool_percent",
    what: "value",
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ZswapParams {
    pub enabled: bool,
    pub compressor: Option<Arc<str>>,
    pub max_pool_percent: Option<u8>,
}

/// None when root.sys/module/zswap/parameters does not exist.
pub fn read_zswap(root: &SysRoot) -> Result<Option<ZswapParams>, CollectError> {
    let dir = root.sys_path("module/zswap/parameters");
    match std::fs::metadata(&dir) {
        Ok(_) => {}
        Err(err) if err.kind() == ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    }

    let mut buf = Vec::new();
    let enabled = match read_trimmed(&dir.join("enabled"), &mut buf)? {
        b"Y" => true,
        b"N" => false,
        _ => return Err(ENABLED_ERROR),
    };
    let compressor = read_optional(&dir.join("compressor"), &mut buf)?
        .filter(|c| !c.is_empty())
        .map(|c| Arc::from(String::from_utf8_lossy(c)));
    let max_pool_percent = read_optional(&dir.join("max_pool_percent"), &mut buf)?
        .map(parse_percent)
        .transpose()?;
    Ok(Some(ZswapParams {
        enabled,
        compressor,
        max_pool_percent,
    }))
}

/// A trimmed small file, or None when it does not exist.
fn read_optional<'b>(path: &Path, buf: &'b mut Vec<u8>) -> Result<Option<&'b [u8]>, CollectError> {
    match read_trimmed(path, buf) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(CollectError::NotFound) => Ok(None),
        Err(err) => Err(err),
    }
}

/// Decimal 0..=100.
fn parse_percent(bytes: &[u8]) -> Result<u8, CollectError> {
    std::str::from_utf8(bytes)
        .ok()
        .and_then(|s| s.parse::<u8>().ok())
        .filter(|p| *p <= 100)
        .ok_or(MAX_POOL_ERROR)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_root(files: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let params = dir.path().join("sys/module/zswap/parameters");
        fs::create_dir_all(&params).expect("mkdir");
        for (name, text) in files {
            fs::write(params.join(name), text).expect("write");
        }
        dir
    }

    #[test]
    fn params_ok() {
        let dir = temp_root(&[
            ("enabled", "N\n"),
            ("compressor", "zstd\n"),
            ("max_pool_percent", "20\n"),
        ]);
        assert_eq!(
            read_zswap(&SysRoot::at(dir.path())).unwrap(),
            Some(ZswapParams {
                enabled: false,
                compressor: Some(Arc::from("zstd")),
                max_pool_percent: Some(20),
            })
        );

        // Optional files absent.
        let dir = temp_root(&[("enabled", "Y\n")]);
        assert_eq!(
            read_zswap(&SysRoot::at(dir.path())).unwrap(),
            Some(ZswapParams {
                enabled: true,
                compressor: None,
                max_pool_percent: None,
            })
        );
    }

    #[test]
    fn module_absent() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert_eq!(read_zswap(&SysRoot::at(dir.path())).unwrap(), None);
    }

    #[test]
    fn bad_enabled() {
        let dir = temp_root(&[("enabled", "maybe\n")]);
        assert!(matches!(
            read_zswap(&SysRoot::at(dir.path())),
            Err(CollectError::Parse {
                file: "zswap enabled",
                what: "value"
            })
        ));

        let dir = temp_root(&[("enabled", "Y\n"), ("max_pool_percent", "101\n")]);
        assert!(matches!(
            read_zswap(&SysRoot::at(dir.path())),
            Err(CollectError::Parse { .. })
        ));
    }
}
