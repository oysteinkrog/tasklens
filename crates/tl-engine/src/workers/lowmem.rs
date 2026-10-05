//! Stripped process sweep for `EngineMode::LowMemory` (ADR-004, PROC-098): `stat`, `status`
//! and cgroup `memory.current` every 2 s, plus the memory pressure state.
