use flanforge_core::{LibvirtConfig, NetworkMode, RuntimeBackendConfig};

use super::inspect_base_image;

#[tokio::test]
async fn inspection_verifies_digest_and_qcow2_metadata_without_libvirt() {
    let directory = tempfile::tempdir().unwrap_or_else(|error| unreachable!("temp: {error}"));
    let state_dir = directory.path().join("state");
    let image = directory.path().join("flanforge-base.qcow2");
    std::fs::write(&image, b"fixture-image\n")
        .unwrap_or_else(|error| unreachable!("image: {error}"));
    let manifest = directory.path().join("base.json");
    std::fs::write(
        &manifest,
        include_bytes!("../../../../templates/libvirt/tests/fixtures/image-manifest.json"),
    )
    .unwrap_or_else(|error| unreachable!("manifest: {error}"));
    let qemu_img = flanforge_test_support::executable(
        directory.path(),
        "qemu-img",
        r#"printf '%s\n' '{"format":"qcow2","virtual-size":42949672960,"backing-filename":null}'
"#,
    );

    let mut config = (*flanforge_test_support::config(state_dir.clone())).clone();
    config.runtime.backend = RuntimeBackendConfig::Libvirt(LibvirtConfig {
        image_manifest_dir: state_dir.join("libvirt/published-bases"),
        qemu_img_path: qemu_img,
        ..LibvirtConfig::default()
    });
    config.runtime.max_running_vms = 1;
    config.runtime.host_cpu_count = Some(8);
    config.runtime.host_memory_mb = Some(32_768);
    config.runtime.host_storage_mb = Some(262_144);
    let ssh = config
        .guest
        .ssh
        .as_mut()
        .unwrap_or_else(|| unreachable!("fixture has [guest.ssh]"));
    ssh.known_hosts_file = None;
    ssh.host_key_alias = None;
    config
        .profiles
        .values_mut()
        .for_each(|profile| profile.network = NetworkMode::Default);

    let report = inspect_base_image(&config, &image, Some(&manifest))
        .await
        .unwrap_or_else(|error| unreachable!("inspect: {error}"));
    assert_eq!(report.file(), "flanforge-base.qcow2");
    assert_eq!(report.format(), "qcow2");
    assert_eq!(report.image_bytes(), 14);
    assert_eq!(report.virtual_bytes(), 42_949_672_960);
}
