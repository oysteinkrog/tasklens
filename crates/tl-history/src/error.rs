//! Errors of the history store.

/// Every way reading or writing history can fail. No message holds a command line.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("i/o error at {path}: {source}")]
    Io {
        path: std::path::PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("bad segment header")]
    BadHeader,
    #[error("unsupported format version {0}")]
    Version(u16),
    #[error("block crc mismatch at offset {0}")]
    Crc(u64),
    #[error("block truncated at offset {0}")]
    Truncated(u64),
    #[error("zstd: {0}")]
    Zstd(std::io::Error),
    #[error("postcard: {0}")]
    Decode(#[from] postcard::Error),
    #[error("load budget exceeded")]
    Timeout,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_messages() {
        assert_eq!(
            HistoryError::Crc(128).to_string(),
            "block crc mismatch at offset 128"
        );
        assert_eq!(
            HistoryError::Truncated(64).to_string(),
            "block truncated at offset 64"
        );
        assert_eq!(
            HistoryError::Version(9).to_string(),
            "unsupported format version 9"
        );
        assert_eq!(HistoryError::BadHeader.to_string(), "bad segment header");
    }
}
