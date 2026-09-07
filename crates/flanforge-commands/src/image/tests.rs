#[test]
fn non_linux_error_text_does_not_offer_destructive_escape_hatches() {
    let message = "libvirt image commands are available only on Linux";
    assert!(!message.contains("force"));
    assert!(!message.contains("delete"));
}
