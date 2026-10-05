//! Sweep cost on the `owner-desktop` processes scaled to 2,000 (ADR-016, ADR-014). Criterion
//! only reports; the 15 ms gate is `tests/fixture_process.rs::sweep_2000_under_gate`.

use criterion::{Criterion, criterion_group, criterion_main};

#[path = "../tests/common/scaled_root.rs"]
mod scaled_root;

fn sweep(c: &mut Criterion) {
    let mut scaled = scaled_root::scaled();
    let mut group = c.benchmark_group("sweep");
    group.bench_function("2000_steady", |b| b.iter(|| scaled.sweep(false)));
    group.bench_function("2000_with_fd_count", |b| b.iter(|| scaled.sweep(true)));
    group.finish();
}

criterion_group!(benches, sweep);
criterion_main!(benches);
