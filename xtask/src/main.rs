//! Developer tasks for tasklens. Not shipped.
#![allow(clippy::print_stdout)]

mod bench;
mod capture_fixture;
mod check_deps;
mod check_timing;

fn main() -> anyhow::Result<()> {
    dispatch(std::env::args().skip(1).collect())
}

/// Runs the subcommand named by the first argument with the remaining arguments.
fn dispatch(args: Vec<String>) -> anyhow::Result<()> {
    let mut args = args.into_iter();
    let cmd = args.next();
    let rest: Vec<String> = args.collect();
    match cmd.as_deref() {
        Some("bench") => bench::run(bench::parse_args(&rest)?),
        Some("capture-fixture") => capture_fixture::run(&rest),
        Some("check-deps") => check_deps::run(rest),
        Some("check-timing") => check_timing::run(rest),
        Some(other) => anyhow::bail!("unknown xtask subcommand: {other}"),
        None => anyhow::bail!("usage: cargo xtask <subcommand> [args]"),
    }
}

#[cfg(test)]
mod tests {
    use super::dispatch;

    #[test]
    fn unknown_subcommand_errors() {
        let err = dispatch(vec!["nope".to_owned()]).unwrap_err();
        assert_eq!(err.to_string(), "unknown xtask subcommand: nope");
    }

    #[test]
    fn missing_subcommand_prints_usage() {
        let err = dispatch(Vec::new()).unwrap_err();
        assert_eq!(err.to_string(), "usage: cargo xtask <subcommand> [args]");
    }
}
