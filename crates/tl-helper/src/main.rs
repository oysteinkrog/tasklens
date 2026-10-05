//! tl-helper: the privileged helper that pkexec runs as root (ADR-006, R-003).
//!
//! The only accepted invocation is `tl-helper --session`. There is no `--once` mode and no other
//! flag. The session loop arrives with E29-A1.

mod dropin;
mod ops;
mod sockets;

use std::process::ExitCode;

const USAGE: &str = "usage: tl-helper --session";

/// Parses the arguments after the program name. `Ok` only for exactly `["--session"]`.
fn parse_args(args: &[&str]) -> Result<(), ()> {
    match args {
        ["--session"] => Ok(()),
        _ => Err(()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    if parse_args(&args).is_err() {
        eprintln!("{USAGE}");
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn args_require_session() {
        assert_eq!(parse_args(&["--session"]), Ok(()));
        assert_eq!(parse_args(&[]), Err(()));
        assert_eq!(parse_args(&["--once"]), Err(()));
        assert_eq!(parse_args(&["--session", "x"]), Err(()));
    }
}
