//! The text of the tasklens recovery drop-in `50-tasklens-recovery.conf` (SYS-013, R-248).
//!
//! One renderer for both paths (R-168 E29-OI-10): the root helper writes the system-unit file and
//! the engine writes the user-unit file, so both get the same text. The caller validates the
//! values first; `render` writes them verbatim.

use tl_model::helper_proto::DropInKey;

/// The first line of every drop-in tasklens writes.
pub const HEADER: &str =
    "# Written by tasklens. Edit it in the Service Properties Recovery tab or delete this file.\n";

/// The `[Unit]` keys in the order they are written.
const UNIT_KEYS: [DropInKey; 5] = [
    DropInKey::StartLimitIntervalSec,
    DropInKey::StartLimitBurst,
    DropInKey::StartLimitAction,
    DropInKey::FailureAction,
    DropInKey::OnFailure,
];

/// The `[Service]` keys in the order they are written.
const SERVICE_KEYS: [DropInKey; 4] = [
    DropInKey::Restart,
    DropInKey::RestartSec,
    DropInKey::RestartSteps,
    DropInKey::RestartMaxDelaySec,
];

/// The drop-in text for `keys`: `HEADER`, then `[Unit]` and `[Service]` with only the keys
/// present, in a fixed order, one `Key=value` line each. A section with no keys is left out.
/// Each key should appear at most once; the caller has checked that every value is one line.
pub fn render(keys: &[(DropInKey, String)]) -> String {
    let mut text = String::from(HEADER);
    for (section, order) in [("[Unit]", &UNIT_KEYS[..]), ("[Service]", &SERVICE_KEYS[..])] {
        let lines: Vec<String> = order
            .iter()
            .flat_map(|want| {
                keys.iter()
                    .filter(move |(key, _)| key == want)
                    .map(|(key, value)| format!("{}={value}\n", key.as_str()))
            })
            .collect();
        if !lines.is_empty() {
            text.push_str(section);
            text.push('\n');
            text.extend(lines);
        }
    }
    text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(pairs: &[(DropInKey, &str)]) -> Vec<(DropInKey, String)> {
        pairs.iter().map(|(k, v)| (*k, v.to_string())).collect()
    }

    #[test]
    fn render_service_only() {
        let text = render(&keys(&[
            (DropInKey::Restart, "on-failure"),
            (DropInKey::RestartSec, "100000us"),
        ]));
        assert_eq!(
            text,
            format!("{HEADER}[Service]\nRestart=on-failure\nRestartSec=100000us\n")
        );
    }

    #[test]
    fn render_unit_only() {
        let text = render(&keys(&[
            (DropInKey::StartLimitBurst, "5"),
            (DropInKey::StartLimitIntervalSec, "10000000us"),
            (DropInKey::OnFailure, "failure-notify@%n.service"),
        ]));
        assert_eq!(
            text,
            format!(
                "{HEADER}[Unit]\nStartLimitIntervalSec=10000000us\nStartLimitBurst=5\n\
                 OnFailure=failure-notify@%n.service\n"
            )
        );
    }

    #[test]
    fn render_all_keys_in_fixed_order() {
        let text = render(&keys(&[
            (DropInKey::RestartMaxDelaySec, "4us"),
            (DropInKey::OnFailure, "a.service b.target"),
            (DropInKey::RestartSteps, "3"),
            (DropInKey::FailureAction, "reboot"),
            (DropInKey::RestartSec, "2us"),
            (DropInKey::StartLimitAction, "none"),
            (DropInKey::Restart, "always"),
            (DropInKey::StartLimitBurst, "1"),
            (DropInKey::StartLimitIntervalSec, "9us"),
        ]));
        assert_eq!(
            text,
            format!(
                "{HEADER}[Unit]\nStartLimitIntervalSec=9us\nStartLimitBurst=1\n\
                 StartLimitAction=none\nFailureAction=reboot\nOnFailure=a.service b.target\n\
                 [Service]\nRestart=always\nRestartSec=2us\nRestartSteps=3\n\
                 RestartMaxDelaySec=4us\n"
            )
        );
    }

    #[test]
    fn render_empty_is_header_only() {
        assert_eq!(render(&[]), HEADER);
    }
}
