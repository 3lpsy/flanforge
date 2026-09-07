use super::*;

#[test]
fn user_info_round_trips() {
    let info = WebuiUserInfo {
        id: 7,
        username: "jim".to_owned(),
        auth_source: "authdb".to_owned(),
        created_at_unix: 1_000,
    };
    let text = serde_json::to_string(&info).unwrap_or_else(|error| unreachable!("{error}"));
    let back: WebuiUserInfo =
        serde_json::from_str(&text).unwrap_or_else(|error| unreachable!("{error}"));
    assert_eq!(back, info);
}
