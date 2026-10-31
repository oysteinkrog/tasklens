//! Every command-line argument, declared once (APP-052, table (c)).
//!
//! `main.rs` holds the arms that act on them. Environment variables are not read here
//! (`TASKLENS_DEBUG` is read in `main.rs`).

#[cfg(feature = "bench")]
use std::path::PathBuf;

use clap::Parser;

/// Parsed command line.
#[derive(clap::Parser, Debug, Clone, PartialEq)]
#[command(name = "tasklens", version, about = "Task manager for Linux")]
pub struct Args {
    #[arg(
        long,
        help = "Start hidden to the tray, or minimized when the tray is off"
    )]
    pub start_minimized: bool,
    #[arg(long, help = "Open the low-memory rescue window")]
    pub low_memory: bool,
    #[arg(long, help = "Run the low-memory watcher (no window)")]
    pub watch_memory: bool,
    #[arg(long, value_enum, help = "Open on this tab")]
    pub tab: Option<TabArg>,
    #[arg(
        long,
        value_name = "PID",
        help = "Open Processes and select this process"
    )]
    pub select_pid: Option<u32>,
    #[arg(long, help = "Show the Debug menu")]
    pub debug: bool,
    #[arg(long, help = "Test option: with --low-memory, run the full engine")]
    pub force_full_memory_mode: bool,
    #[arg(
        long,
        value_name = "ID",
        help = "Run a separate instance with its own settings, state, cache and bus name (development builds)"
    )]
    pub app_id: Option<String>,
    #[command(subcommand)]
    pub command: Option<Cmd>,
    #[cfg(feature = "bench")]
    #[arg(long, value_enum, value_name = "SCENE", help = "Run a benchmark scene")]
    pub bench: Option<BenchScene>,
    #[cfg(feature = "bench")]
    #[arg(long, requires = "bench", value_name = "SECONDS")]
    pub duration: Option<u32>,
    #[cfg(feature = "bench")]
    #[arg(long, requires = "bench", value_name = "SECONDS")]
    pub warmup: Option<u32>,
    #[cfg(feature = "bench")]
    #[arg(long, requires = "bench", value_name = "PATH")]
    pub json: Option<PathBuf>,
    #[cfg(feature = "bench")]
    #[arg(long, conflicts_with = "bench", help = "Open the component gallery")]
    pub gallery: bool,
}

/// Subcommands.
#[derive(clap::Subcommand, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cmd {
    /// Connect stdin and stdout to this user's engine service (the far end of ssh, ADR-018)
    Bridge,
    /// Let tasklens windows on other PCs run administrator actions here without a password
    /// prompt, or stop that, or show whether it is on (ADR-018)
    RemoteAdmin {
        #[arg(value_enum)]
        what: RemoteAdminArg,
    },
}

/// Values of `tasklens remote-admin`.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteAdminArg {
    On,
    Off,
    Status,
}

/// Values of `--tab`.
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum TabArg {
    Processes,
    Performance,
    Services,
    Startup,
    Connections,
    Information,
}

impl TabArg {
    /// The tab type used by `AppRequest::SelectTab` (R-136).
    pub fn to_model(self) -> tl_model::command::Tab {
        use tl_model::command::Tab;
        match self {
            TabArg::Processes => Tab::Processes,
            TabArg::Performance => Tab::Performance,
            TabArg::Services => Tab::Services,
            TabArg::Startup => Tab::Startup,
            TabArg::Connections => Tab::Connections,
            TabArg::Information => Tab::Information,
        }
    }
}

/// Values of `--bench`. CLI names: perf-cpu, perf-overview, perf-overview-static,
/// perf-overview-procs, table-2000, bridge, static, history-scrub, soak.
#[cfg(feature = "bench")]
#[derive(clap::ValueEnum, Debug, Clone, Copy, PartialEq, Eq)]
pub enum BenchScene {
    PerfCpu,
    PerfOverview,
    PerfOverviewStatic,
    PerfOverviewProcs,
    #[value(name = "table-2000")]
    Table2000,
    Bridge,
    Static,
    HistoryScrub,
    Soak,
}

/// Parses the process arguments. On a usage error clap prints usage to stderr and exits 2.
/// `--help` and `--version` print to stdout and exit 0.
pub fn parse() -> Args {
    Args::parse()
}

#[cfg(test)]
mod tests {
    use clap::error::ErrorKind;

    use super::*;

    fn parse_from(argv: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("tasklens").chain(argv.iter().copied()))
    }

    #[test]
    fn tab_and_pid() {
        let args = parse_from(&["--tab", "services", "--select-pid", "1234"]).unwrap();
        assert_eq!(args.tab, Some(TabArg::Services));
        assert_eq!(args.select_pid, Some(1234));
    }

    #[test]
    fn unknown_argument_is_error() {
        let err = parse_from(&["--frobnicate"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::UnknownArgument);
        assert_eq!(err.exit_code(), 2);
        assert!(err.use_stderr());
    }

    #[test]
    fn bad_tab_value() {
        let err = parse_from(&["--tab", "nowhere"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn negative_pid_rejected() {
        let err = parse_from(&["--select-pid", "-5"]).unwrap_err();
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn version_flag() {
        let err = parse_from(&["--version"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::DisplayVersion);
        assert_eq!(err.exit_code(), 0);
        assert!(!err.use_stderr());
        assert_eq!(
            err.render().to_string(),
            format!("tasklens {}\n", env!("CARGO_PKG_VERSION"))
        );
    }

    #[test]
    fn all_bools_default_false() {
        let args = parse_from(&[]).unwrap();
        assert!(!args.start_minimized);
        assert!(!args.low_memory);
        assert!(!args.watch_memory);
        assert!(!args.debug);
        assert!(!args.force_full_memory_mode);
        assert_eq!(args.tab, None);
        assert_eq!(args.select_pid, None);
        assert_eq!(args.app_id, None);
        assert_eq!(args.command, None);
        #[cfg(feature = "bench")]
        assert!(!args.gallery);
    }

    #[test]
    fn bridge_subcommand() {
        assert_eq!(parse_from(&["bridge"]).unwrap().command, Some(Cmd::Bridge));
        let args = parse_from(&["--app-id", "dev", "bridge"]).unwrap();
        assert_eq!(args.command, Some(Cmd::Bridge));
        assert_eq!(args.app_id.as_deref(), Some("dev"));
    }

    #[test]
    fn remote_admin_subcommand() {
        for (word, what) in [
            ("on", RemoteAdminArg::On),
            ("off", RemoteAdminArg::Off),
            ("status", RemoteAdminArg::Status),
        ] {
            let args = parse_from(&["remote-admin", word]).unwrap();
            assert_eq!(args.command, Some(Cmd::RemoteAdmin { what }));
        }
        let err = parse_from(&["remote-admin"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert_eq!(err.exit_code(), 2);
        let err = parse_from(&["remote-admin", "maybe"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::InvalidValue);
        assert_eq!(err.exit_code(), 2);
    }

    #[test]
    fn tab_maps_to_model() {
        use clap::ValueEnum;
        use tl_model::command::Tab;
        let pairs = [
            ("processes", Tab::Processes),
            ("performance", Tab::Performance),
            ("services", Tab::Services),
            ("startup", Tab::Startup),
            ("connections", Tab::Connections),
            ("information", Tab::Information),
        ];
        assert_eq!(TabArg::value_variants().len(), pairs.len());
        for (name, tab) in pairs {
            let arg = parse_from(&["--tab", name]).unwrap().tab.unwrap();
            assert_eq!(arg.to_model(), tab);
        }
    }

    #[cfg(not(feature = "bench"))]
    #[test]
    fn bench_is_unknown_without_feature() {
        for argv in [&["--bench", "perf-cpu"][..], &["--gallery"][..]] {
            let err = parse_from(argv).unwrap_err();
            assert_eq!(err.kind(), ErrorKind::UnknownArgument);
            assert_eq!(err.exit_code(), 2);
        }
    }

    #[cfg(feature = "bench")]
    #[test]
    fn bench_scene_names() {
        let pairs = [
            ("perf-cpu", BenchScene::PerfCpu),
            ("perf-overview", BenchScene::PerfOverview),
            ("perf-overview-static", BenchScene::PerfOverviewStatic),
            ("perf-overview-procs", BenchScene::PerfOverviewProcs),
            ("table-2000", BenchScene::Table2000),
            ("bridge", BenchScene::Bridge),
            ("static", BenchScene::Static),
            ("history-scrub", BenchScene::HistoryScrub),
            ("soak", BenchScene::Soak),
        ];
        for (name, scene) in pairs {
            assert_eq!(parse_from(&["--bench", name]).unwrap().bench, Some(scene));
        }
        let args = parse_from(&["--bench", "table-2000", "--duration", "30"]).unwrap();
        assert_eq!(args.bench, Some(BenchScene::Table2000));
        assert_eq!(args.duration, Some(30));
    }

    #[cfg(feature = "bench")]
    #[test]
    fn duration_requires_bench() {
        let err = parse_from(&["--duration", "30"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::MissingRequiredArgument);
        assert_eq!(err.exit_code(), 2);
    }

    #[cfg(feature = "bench")]
    #[test]
    fn gallery_conflicts_with_bench() {
        let err = parse_from(&["--gallery", "--bench", "static"]).unwrap_err();
        assert_eq!(err.kind(), ErrorKind::ArgumentConflict);
        assert_eq!(err.exit_code(), 2);
    }
}
