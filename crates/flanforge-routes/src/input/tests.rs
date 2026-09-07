use validator::Validate;

use flanforge_core::HotRequest;

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

/// One field, four meanings. The string arm is closed to one word, so a
/// misspelled action is a bad request rather than a silently ignored setting.
#[test]
fn the_hot_field_decodes_exactly_four_meanings() {
    for (json, expected) in [
        (r#""hot":false"#, HotRequest::Untouched),
        (r#""hot":true"#, HotRequest::Retain { age_seconds: None }),
        (
            r#""hot":3600"#,
            HotRequest::Retain {
                age_seconds: Some(3_600),
            },
        ),
        (r#""hot":"evict""#, HotRequest::Evict),
    ] {
        let document = format!(
            r#"{{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,{json}}}"#
        );
        let (_, options) = body(&document)
            .unwrap_or_else(|| unreachable!("valid body: {json}"))
            .into_parts()
            .unwrap_or_else(|_| unreachable!("valid body: {json}"));
        assert_eq!(options.hot, expected, "{json}");
    }

    for invalid in [
        // Zero is not a synonym for false: a zero-second machine is incoherent.
        r#""hot":0"#,
        r#""hot":"drain""#,
        r#""hot":"true""#,
        r#""hot":604801"#,
        r#""hot":{"enabled":true}"#,
    ] {
        let document = format!(
            r#"{{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,{invalid}}}"#
        );
        let refused = body(&document).is_none_or(|body| body.into_parts().is_err());
        assert!(refused, "accepted {invalid}");
    }
}

/// A retaining request sources like a warm one, so hot on a profile with no
/// warm template still falls back to the cold template rather than failing.
#[test]
fn a_retaining_request_carries_no_warm_flag_of_its_own() {
    let (_, options) = body(
        r#"{"profile":"halogen","repository":"owner/halogen","run_id":42,"run_attempt":1,"hot":true}"#,
    )
    .unwrap_or_else(|| unreachable!("valid body"))
    .into_parts()
    .unwrap_or_else(|_| unreachable!("valid body"));
    assert!(!options.warm);
    assert!(options.hot.is_retaining());
}
