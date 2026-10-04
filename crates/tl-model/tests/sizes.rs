//! Size checks for the core types (ADR-003 "Verification"). History files and the hot path
//! depend on these numbers.

use std::mem::size_of;

#[test]
fn sample_is_16_bytes() {
    assert_eq!(size_of::<tl_model::series::Sample>(), 16);
}

#[test]
fn process_key_is_24_bytes() {
    assert_eq!(size_of::<tl_model::ids::ProcessKey>(), 24);
}

#[test]
fn process_record_at_most_160() {
    let size = size_of::<tl_model::process::ProcessRecord>();
    assert!(
        size <= 160,
        "ProcessRecord is {size} bytes, the limit is 160"
    );
}

#[test]
fn field_validity_is_13_bytes() {
    assert_eq!(size_of::<tl_model::validity::FieldValidity>(), 13);
}

#[test]
fn validity_is_1_byte() {
    assert_eq!(size_of::<tl_model::validity::Validity>(), 1);
}

#[test]
fn cpuset_is_128_bytes() {
    assert_eq!(size_of::<tl_model::system::CpuSet>(), 128);
}
