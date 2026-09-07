use validator::Validate;

use super::CapacityStatus;

#[test]
fn capacity_status_rejects_impossible_wire_values() {
    let mut status = CapacityStatus {
        active_allocations: 1,
        foreign_running: 0,
        hot_running: 1,
        max_hot_vms: 1,
        max_running_vms: 1,
        committed_cpu_count: 2,
        committed_memory_mb: 4_096,
        committed_storage_mb: 40_960,
        host_cpu_count: Some(8),
        host_memory_mb: Some(16_384),
        host_storage_mb: Some(262_144),
        is_host_visible: true,
    };
    assert!(status.validate().is_ok());
    status.max_running_vms = 0;
    assert!(status.validate().is_err());
}

/// The two hot fields are `serde(default)`, so a pre-hot daemon's payload
/// still decodes rather than failing the whole status read.
#[test]
fn capacity_status_decodes_a_payload_written_before_hot_occupancy() {
    let document = r#"{"active_allocations":1,"foreign_running":0,"max_running_vms":2,
        "committed_cpu_count":2,"committed_memory_mb":4096,"committed_storage_mb":40960,
        "host_cpu_count":8,"host_memory_mb":16384,"host_storage_mb":262144,
        "is_host_visible":true}"#;
    let status = serde_json::from_str::<CapacityStatus>(document)
        .unwrap_or_else(|error| unreachable!("decode: {error}"));
    assert_eq!(status.hot_running, 0);
    assert_eq!(status.max_hot_vms, 0);
}
