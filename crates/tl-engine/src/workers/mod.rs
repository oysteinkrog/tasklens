//! The engine's worker registry (ADR-004).
//!
//! Each worker bead adds one `pub mod <worker>;` line and one `WorkerSpec` in `all_workers`
//! (chain edit). The logind worker (E08-A2) is the first entry.

pub mod cpu;
mod dbus_systemd;
pub mod disk;
pub mod enumerate;
pub mod gpu_nvidia;
pub mod journal;
pub mod logind;
pub mod memory;
pub mod net;
pub mod processes;
pub mod sensors;

use std::sync::Arc;

use tl_model::health::SourceId;

use crate::config::{EngineConfig, EngineMode};
use crate::source::WorkerIo;
use crate::supervise::WorkerFactory;

/// One worker: its source, its log name, the modes it runs in and how to start it.
pub struct WorkerSpec {
    pub id: SourceId,
    pub name: &'static str,
    pub modes: &'static [EngineMode],
    pub factory: fn(Arc<EngineConfig>, WorkerIo) -> WorkerFactory,
}

/// Every worker of the engine. Each worker bead appends exactly one `WorkerSpec` here.
pub fn all_workers() -> Vec<WorkerSpec> {
    vec![
        logind::spec(),
        memory::spec(),
        disk::spec(),
        net::spec(),
        sensors::spec(),
        cpu::spec(),
        processes::spec(),
        gpu_nvidia::spec(),
        dbus_systemd::spec_system(),
        dbus_systemd::spec_user(),
    ]
}

/// The workers that run in `mode`.
pub fn workers_for(mode: EngineMode) -> Vec<WorkerSpec> {
    all_workers()
        .into_iter()
        .filter(|spec| spec.modes.contains(&mode))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workers_for_filters_by_mode() {
        let all = all_workers();
        for mode in [EngineMode::Full, EngineMode::LowMemory] {
            let ids: Vec<SourceId> = workers_for(mode).iter().map(|spec| spec.id).collect();
            let expected: Vec<SourceId> = all
                .iter()
                .filter(|spec| spec.modes.contains(&mode))
                .map(|spec| spec.id)
                .collect();
            assert_eq!(ids, expected, "{mode:?}");
        }
        // E32's low-memory arm changes the sweep, not the set.
        let low: Vec<SourceId> = workers_for(EngineMode::LowMemory)
            .iter()
            .map(|spec| spec.id)
            .collect();
        assert_eq!(low, [SourceId::Processes]);
        assert_eq!(
            workers_for(EngineMode::Full)
                .iter()
                .filter(|spec| spec.id == SourceId::Cpu && spec.name == "cpu")
                .count(),
            1
        );
        assert_eq!(
            workers_for(EngineMode::Full)
                .iter()
                .filter(|spec| spec.id == SourceId::GpuNvidia)
                .count(),
            1
        );
    }
}
