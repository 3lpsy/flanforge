use virt::error::ErrorNumber;

use crate::RuntimeError;

use super::create_error;

#[test]
fn preexisting_volume_is_an_explicit_non_ownership_collision() {
    assert!(matches!(
        create_error("overlay creation", ErrorNumber::StorageVolExist, "exists"),
        RuntimeError::Collision { .. }
    ));
    assert!(matches!(
        create_error("overlay creation", ErrorNumber::OperationFailed, "failed"),
        RuntimeError::Libvirt { .. }
    ));
}
