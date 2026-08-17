use validator::Validate;

use super::CreateBody;

fn body(json: &str) -> Option<CreateBody> {
    let body = serde_json::from_str::<CreateBody>(json).ok()?;
    body.validate().ok()?;
    Some(body)
}

#[test]
fn create_body_rejects_an_unknown_field_and_an_out_of_range_size() {
    let valid = body(
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":"42","run_attempt":1,
            "warm":true,"cpu_count":2,"memory_mb":4096}"#,
    )
    .unwrap_or_else(|| unreachable!("valid body"));
    let (request, options) = valid
        .into_parts()
        .unwrap_or_else(|_| unreachable!("valid body"));
    assert_eq!(request.run_id, 42);
    assert!(options.warm);
    assert_eq!(options.cpu_count, Some(2));
    assert_eq!(options.memory_mb, Some(4_096));

    for invalid in [
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"template":"base"}"#,
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"cpu_count":65}"#,
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"cpu_count":0}"#,
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"memory_mb":2047}"#,
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"memory_mb":131073}"#,
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":0,"run_attempt":1}"#,
    ] {
        assert!(body(invalid).is_none(), "accepted {invalid}");
    }
}

#[test]
fn an_unsized_cold_request_keeps_todays_shape() {
    let body =
        body(r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1}"#)
            .unwrap_or_else(|| unreachable!("valid body"));
    let (_, options) = body
        .into_parts()
        .unwrap_or_else(|_| unreachable!("valid body"));
    assert_eq!(options, flanforge_core::RequestOptions::default());
}
