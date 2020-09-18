use bench_utils::api::xfr::xfr_note_idtracking_noassettracking_singleasset;
use criterion::measurement::WallTime;
use criterion::{criterion_group, criterion_main, Criterion};

// Benchmark with time
criterion_group!(
    name = xfr_note_idtracking_noassettracking_singleasset_with_time;
    config = Criterion::default().with_measurement(WallTime);
    targets = xfr_note_idtracking_noassettracking_singleasset::<WallTime>
);
criterion_main!(xfr_note_idtracking_noassettracking_singleasset_with_time);
