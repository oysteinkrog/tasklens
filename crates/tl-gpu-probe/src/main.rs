//! tl-gpu-probe: the only process that loads NVML.
//!
//! The engine starts this binary with a socket on fd 3 and talks to it with `gpu_proto` frames
//! (ADR-007). An NVML crash or hang ends only this process.

use std::process::ExitCode;

mod sampler;

const USAGE: &str = "usage: tl-gpu-probe --fd <n> | --api-versions | --fake <script> [--fd <n>]";

/// fd that the `--fake` mode uses when `--fd` is not given.
#[cfg(feature = "fake")]
const DEFAULT_FAKE_FD: i32 = 3;

#[derive(Debug, PartialEq)]
pub enum Mode {
    Sampler {
        fd: i32,
    },
    ApiVersions,
    #[cfg(feature = "fake")]
    Fake {
        script: String,
        fd: i32,
    },
}

/// Parses the arguments after the program name. Anything not listed in `USAGE` is an error.
pub fn parse_args(args: &[String]) -> Result<Mode, String> {
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    match args.as_slice() {
        ["--fd", n] => Ok(Mode::Sampler { fd: parse_fd(n)? }),
        ["--api-versions"] => Ok(Mode::ApiVersions),
        #[cfg(feature = "fake")]
        ["--fake", script] => Ok(Mode::Fake {
            script: (*script).to_owned(),
            fd: DEFAULT_FAKE_FD,
        }),
        #[cfg(feature = "fake")]
        ["--fake", script, "--fd", n] => Ok(Mode::Fake {
            script: (*script).to_owned(),
            fd: parse_fd(n)?,
        }),
        _ => Err(USAGE.to_owned()),
    }
}

fn parse_fd(s: &str) -> Result<i32, String> {
    match s.parse::<i32>() {
        Ok(fd) if fd >= 0 => Ok(fd),
        _ => Err(USAGE.to_owned()),
    }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match parse_args(&args) {
        Err(usage) => {
            eprintln!("{usage}");
            ExitCode::from(2)
        }
        // E13-002 (ndy.4) replaces this arm with sampler::run(fd, nvml::NvmlBackend::new).
        Ok(Mode::Sampler { fd: _ }) => {
            eprintln!("nvml backend not built");
            ExitCode::from(3)
        }
        // E13-006 replaces this arm with api_versions::run().
        Ok(Mode::ApiVersions) => {
            eprintln!("api-versions not built");
            ExitCode::from(3)
        }
        // E13-005 (ndy.7) replaces this arm with sampler::run(fd, fake::FakeBackend::from_script).
        #[cfg(feature = "fake")]
        Ok(Mode::Fake { .. }) => {
            eprintln!("fake backend not built");
            ExitCode::from(3)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn parse_args_table() {
        assert_eq!(
            parse_args(&args(&["--fd", "3"])),
            Ok(Mode::Sampler { fd: 3 })
        );
        assert_eq!(
            parse_args(&args(&["--api-versions"])),
            Ok(Mode::ApiVersions)
        );
        assert!(parse_args(&args(&[])).is_err());
        assert!(parse_args(&args(&["--fd", "x"])).is_err());
        assert!(parse_args(&args(&["--fd", "-1"])).is_err());
        assert!(parse_args(&args(&["--fd"])).is_err());
        assert!(parse_args(&args(&["--fd", "3", "--api-versions"])).is_err());
        assert!(parse_args(&args(&["--bogus"])).is_err());

        #[cfg(feature = "fake")]
        {
            assert_eq!(
                parse_args(&args(&["--fake", "normal"])),
                Ok(Mode::Fake {
                    script: "normal".to_owned(),
                    fd: 3
                })
            );
            assert_eq!(
                parse_args(&args(&["--fake", "crash", "--fd", "5"])),
                Ok(Mode::Fake {
                    script: "crash".to_owned(),
                    fd: 5
                })
            );
            assert!(parse_args(&args(&["--fake"])).is_err());
        }
        #[cfg(not(feature = "fake"))]
        assert!(parse_args(&args(&["--fake", "normal"])).is_err());
    }
}
