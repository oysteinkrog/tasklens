//! The build facts About shows (UI-SHL-092): version, 7-character hash and build date.

use tl_ui::BuildInfo;

/// Version, git hash and build date baked in by `build.rs`.
pub fn build_info() -> BuildInfo {
    BuildInfo {
        version: env!("CARGO_PKG_VERSION"),
        git_hash: env!("TASKLENS_GIT_SHA"),
        build_date: env!("TASKLENS_BUILD_DATE"),
        ..BuildInfo::default()
    }
}

#[cfg(test)]
mod tests {
    use super::build_info;

    #[test]
    fn hash_is_seven_hex_characters_or_unknown() {
        let hash = build_info().git_hash;
        let seven_hex = hash.len() == 7 && hash.bytes().all(|b| b.is_ascii_hexdigit());
        assert!(seven_hex || hash == "unknown", "hash {hash:?}");
    }

    #[test]
    fn date_is_never_empty_and_has_iso_format() {
        let date = build_info().build_date;
        let b = date.as_bytes();
        let iso = b.len() == 10
            && b.iter().enumerate().all(|(i, c)| {
                if i == 4 || i == 7 {
                    *c == b'-'
                } else {
                    c.is_ascii_digit()
                }
            });
        assert!(iso, "date {date:?}");
    }
}
