//! Network sampler: per-interface download and upload rates, totals since boot and connection
//! state (ADR-016, ADR-008, PERF-054 to PERF-056, R-052). Download is rx, upload is tx.
use std::sync::Arc;

use tl_model::series::SampleFlags;
use tl_model::time::TimeNs;
use tl_model::validity::Validity;

use crate::SysRoot;
use crate::counters::{Clock, Counter, Measured, Rate, resumed_between};
use crate::parse::netdev::{NetDevLine, parse_net_dev};
use crate::parse::netif::{OperState, read_oper_state};
use crate::sysroot::KeptFile;

/// One interface as the enumeration saw it. A new `generation` means a new interface with the
/// same name, so its counters start over.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct IfaceId {
    pub name: Arc<str>,
    pub generation: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct NetIfReading {
    pub name: Arc<str>,
    /// Bytes per second; bits are a UI unit choice.
    pub rx_bps: Measured<f64>,
    pub tx_bps: Measured<f64>,
    /// Raw counters since boot (PERF-056).
    pub total_rx_bytes: Measured<u64>,
    pub total_tx_bytes: Measured<u64>,
    pub state: Measured<OperState>,
    pub flags: SampleFlags,
}

impl Default for NetIfReading {
    fn default() -> Self {
        Self {
            name: Arc::from(""),
            rx_bps: Measured::default(),
            tx_bps: Measured::default(),
            total_rx_bytes: Measured::default(),
            total_tx_bytes: Measured::default(),
            state: Measured {
                value: OperState::Unknown,
                validity: Validity::WarmingUp,
            },
            flags: SampleFlags::empty(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct NetReading {
    pub t_ns: TimeNs,
    /// Of the `/proc/net/dev` read.
    pub validity: Validity,
    /// In the order of `set_interfaces`.
    pub ifaces: Vec<NetIfReading>,
}

/// One interface with its rx and tx byte counters.
struct IfaceState {
    id: IfaceId,
    rx: Counter,
    tx: Counter,
}

pub struct NetSampler {
    root: SysRoot,
    dev: KeptFile,
    buf: Vec<u8>,
    lines: Vec<NetDevLine>,
    ifaces: Vec<IfaceState>,
    prev_clock: Option<Clock>,
    /// Set by a resume or `reset_baselines`. Cleared by the next successful read.
    gap_pending: bool,
}

impl NetSampler {
    pub fn new(root: SysRoot) -> Self {
        let dev = KeptFile::new(root.proc_path("net/dev"));
        Self {
            root,
            dev,
            buf: Vec::new(),
            lines: Vec::new(),
            ifaces: Vec::new(),
            prev_clock: None,
            gap_pending: false,
        }
    }

    /// Call after each enumeration. An interface whose name is still present keeps its
    /// counters; a changed generation makes its next rates `WarmingUp` with `GAP_BEFORE`.
    pub fn set_interfaces(&mut self, ifaces: &[IfaceId]) {
        let mut old = std::mem::take(&mut self.ifaces);
        self.ifaces = ifaces
            .iter()
            .map(|id| {
                let (rx, tx) = old
                    .iter()
                    .position(|s| s.id.name == id.name)
                    .map(|i| {
                        let s = old.swap_remove(i);
                        (s.rx, s.tx)
                    })
                    .unwrap_or_default();
                IfaceState {
                    id: id.clone(),
                    rx,
                    tx,
                }
            })
            .collect();
    }

    pub fn sample(&mut self, out: &mut NetReading) {
        self.sample_at(Clock::now(), out);
    }

    pub fn sample_at(&mut self, clock: Clock, out: &mut NetReading) {
        if let Some(prev) = self.prev_clock
            && resumed_between(prev, clock)
        {
            self.gap_pending = true;
        }
        self.prev_clock = Some(clock);
        out.t_ns = clock.boot_ns;

        let read = self
            .dev
            .read(&mut self.buf)
            .and_then(|_| parse_net_dev(&self.buf, &mut self.lines));
        out.validity = match &read {
            Ok(()) => Validity::Valid,
            Err(err) => err.validity(),
        };
        let gap = read.is_ok() && self.gap_pending;
        if gap {
            self.gap_pending = false;
            for state in &mut self.ifaces {
                state.rx.reset();
                state.tx.reset();
            }
        }

        out.ifaces.truncate(self.ifaces.len());
        out.ifaces
            .resize_with(self.ifaces.len(), NetIfReading::default);
        let t = clock.boot_ns;
        for (state, reading) in self.ifaces.iter_mut().zip(out.ifaces.iter_mut()) {
            if !Arc::ptr_eq(&reading.name, &state.id.name) {
                reading.name = Arc::clone(&state.id.name);
            }
            if read.is_err() {
                reading.rx_bps = Measured::invalid(out.validity);
                reading.tx_bps = Measured::invalid(out.validity);
                reading.total_rx_bytes = Measured::invalid(out.validity);
                reading.total_tx_bytes = Measured::invalid(out.validity);
                reading.flags = SampleFlags::empty();
                reading.state = read_oper_state(&self.root, &state.id.name, &mut self.buf);
                continue;
            }
            let name = state.id.name.as_bytes();
            let Some(line) = self.lines.iter().find(|l| l.name.as_slice() == name) else {
                reading.rx_bps = Measured::invalid(Validity::Missing);
                reading.tx_bps = Measured::invalid(Validity::Missing);
                reading.total_rx_bytes = Measured::invalid(Validity::Missing);
                reading.total_tx_bytes = Measured::invalid(Validity::Missing);
                reading.state = Measured {
                    value: OperState::Unknown,
                    validity: Validity::Missing,
                };
                reading.flags = SampleFlags::empty();
                continue;
            };
            let generation = state.id.generation;
            let rx = state.rx.update(line.rx_bytes, t, generation);
            let tx = state.tx.update(line.tx_bytes, t, generation);
            reading.rx_bps = measured_rate(rx);
            reading.tx_bps = measured_rate(tx);
            reading.total_rx_bytes = Measured::valid(line.rx_bytes);
            reading.total_tx_bytes = Measured::valid(line.tx_bytes);
            reading.flags = if gap {
                SampleFlags::GAP_BEFORE
            } else {
                rx.flags | tx.flags
            };
            reading.state = read_oper_state(&self.root, &state.id.name, &mut self.buf);
        }
    }

    /// The next rates are `WarmingUp` and every interface has `flags = GAP_BEFORE`.
    pub fn reset_baselines(&mut self) {
        self.gap_pending = true;
    }
}

fn measured_rate(rate: Rate) -> Measured<f64> {
    Measured {
        value: rate.per_sec,
        validity: rate.validity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::{Path, PathBuf};

    const SEC: u64 = 1_000_000_000;

    fn step_dir(step: u32) -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/net-two-steps")
            .join(format!("step{step}"))
    }

    /// Writes the files of `step` over the files in `to`. Writing in place keeps the inode, so
    /// the sampler's kept descriptor sees the new contents.
    fn load_step(to: &Path, step: u32) {
        fn copy(from: &Path, to: &Path) {
            fs::create_dir_all(to).unwrap();
            for entry in fs::read_dir(from).unwrap() {
                let entry = entry.unwrap();
                let dst = to.join(entry.file_name());
                if entry.file_type().unwrap().is_dir() {
                    copy(&entry.path(), &dst);
                } else {
                    fs::write(&dst, fs::read(entry.path()).unwrap()).unwrap();
                }
            }
        }
        copy(&step_dir(step), to);
    }

    fn at(secs: u64) -> Clock {
        Clock {
            boot_ns: secs * SEC,
            mono_ns: secs * SEC,
        }
    }

    fn eth0(generation: u32) -> IfaceId {
        IfaceId {
            name: Arc::from("eth0"),
            generation,
        }
    }

    fn sampler(dir: &Path, ifaces: &[IfaceId]) -> NetSampler {
        let mut s = NetSampler::new(SysRoot::at(dir));
        s.set_interfaces(ifaces);
        s
    }

    #[test]
    fn first_sample_warming_up() {
        let mut s = sampler(&step_dir(0), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        assert_eq!(out.validity, Validity::Valid);
        let r = &out.ifaces[0];
        assert_eq!(&*r.name, "eth0");
        assert_eq!(r.rx_bps.validity, Validity::WarmingUp);
        assert_eq!(r.tx_bps.validity, Validity::WarmingUp);
        assert_eq!(r.total_rx_bytes, Measured::valid(1_000_000));
        assert_eq!(r.total_tx_bytes, Measured::valid(500_000));
        assert_eq!(r.state, Measured::valid(OperState::Connected));
    }

    #[test]
    fn rates_from_two_steps() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        let r = &out.ifaces[0];
        assert_eq!(out.t_ns, SEC);
        assert_eq!(r.rx_bps, Measured::valid(2_500_000.0));
        assert_eq!(r.tx_bps, Measured::valid(1_000_000.0));
        assert_eq!(r.total_rx_bytes, Measured::valid(3_500_000));
        assert_eq!(r.flags, SampleFlags::VALID);
    }

    #[test]
    fn counter_reset() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sampler(dir.path(), &[eth0(0)]);
        let mut out = NetReading::default();
        for step in 0..=2 {
            load_step(dir.path(), step);
            s.sample_at(at(u64::from(step)), &mut out);
        }
        let r = &out.ifaces[0];
        assert_eq!(r.rx_bps.validity, Validity::WarmingUp);
        assert!(r.flags.contains(SampleFlags::RESET));
        assert_eq!(r.total_rx_bytes, Measured::valid(100));
    }

    #[test]
    fn generation_change_gap() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        s.set_interfaces(&[eth0(1)]);
        load_step(dir.path(), 1);
        s.sample_at(at(1), &mut out);
        let r = &out.ifaces[0];
        assert_eq!(r.rx_bps.validity, Validity::WarmingUp);
        assert_eq!(r.tx_bps.validity, Validity::WarmingUp);
        assert!(r.flags.contains(SampleFlags::GAP_BEFORE));
    }

    #[test]
    fn missing_interface() {
        let wlan9 = IfaceId {
            name: Arc::from("wlan9"),
            generation: 0,
        };
        let mut s = sampler(&step_dir(0), &[wlan9, eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        let r = &out.ifaces[0];
        assert_eq!(&*r.name, "wlan9");
        assert_eq!(r.rx_bps.validity, Validity::Missing);
        assert_eq!(r.tx_bps.validity, Validity::Missing);
        assert_eq!(r.total_rx_bytes.validity, Validity::Missing);
        assert_eq!(r.total_tx_bytes.validity, Validity::Missing);
        assert_eq!(r.state.validity, Validity::Missing);
        assert_eq!(&*out.ifaces[1].name, "eth0");
        assert_eq!(out.ifaces[1].total_rx_bytes, Measured::valid(1_000_000));
    }

    #[test]
    fn resume_gap() {
        let dir = tempfile::tempdir().unwrap();
        load_step(dir.path(), 0);
        let mut s = sampler(dir.path(), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        load_step(dir.path(), 1);
        // Boot time moved 10 s, monotonic time 1 s: the machine slept.
        s.sample_at(
            Clock {
                boot_ns: 10 * SEC,
                mono_ns: SEC,
            },
            &mut out,
        );
        let r = &out.ifaces[0];
        assert_eq!(r.rx_bps.validity, Validity::WarmingUp);
        assert_eq!(r.tx_bps.validity, Validity::WarmingUp);
        assert_eq!(r.flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn reset_baselines_gap() {
        let mut s = sampler(&step_dir(0), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        s.reset_baselines();
        s.sample_at(at(1), &mut out);
        assert_eq!(out.ifaces[0].rx_bps.validity, Validity::WarmingUp);
        assert_eq!(out.ifaces[0].flags, SampleFlags::GAP_BEFORE);
    }

    #[test]
    fn read_failure_sets_validity() {
        let dir = tempfile::tempdir().unwrap();
        let mut s = sampler(dir.path(), &[eth0(0)]);
        let mut out = NetReading::default();
        s.sample_at(at(0), &mut out);
        assert_ne!(out.validity, Validity::Valid);
        let r = &out.ifaces[0];
        for v in [r.rx_bps.validity, r.tx_bps.validity] {
            assert_eq!(v, out.validity);
        }
        assert_eq!(r.total_rx_bytes.validity, out.validity);
        assert_eq!(r.total_tx_bytes.validity, out.validity);
    }
}
