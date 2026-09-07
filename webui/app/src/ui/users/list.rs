//! Account admin: the list, with per-row reset and delete. Creation lives on
//! its own page.

use dioxus::prelude::*;
use flanforge_webui_api_client as api;
use flanforge_webui_components::{
    ErrorState, Loading, Pager, SearchBox, Sort, SortableTh, human_time, now_secs, page_slice,
    row_matches,
};
use flanforge_wire::{SetPasswordRequest, WebuiUserInfo};

use crate::routes::Route;
use crate::session::use_session;

#[component]
pub fn Users() -> Element {
    let session = use_session();
    let mut users = use_signal(|| Option::<Result<Vec<WebuiUserInfo>, api::ApiError>>::None);
    let mut notice = use_signal(|| Option::<String>::None);
    let query = use_signal(String::new);
    let page = use_signal(|| 1_usize);
    let per_page = use_signal(|| 25_usize);
    let sort = use_signal(|| ("username", true) as Sort);
    // The account whose password is being reset, with the typed replacement.
    let mut resetting = use_signal(|| Option::<(i64, String)>::None);
    let mut new_password = use_signal(String::new);
    let own_id = session
        .meta()
        .and_then(|meta| meta.user.map(|user| user.id));

    let reload = move || {
        spawn(async move {
            users.set(Some(
                api::get_json::<Vec<WebuiUserInfo>>("/api/v1/users").await,
            ));
        });
    };
    use_future(move || async move {
        reload();
    });

    let delete = move |id: i64| {
        spawn(async move {
            match api::send_empty::<()>("DELETE", &format!("/api/v1/users/{id}"), None).await {
                Ok(()) => reload(),
                Err(error) => notice.set(Some(error.to_string())),
            }
        });
    };

    let confirm_reset = move |_| {
        let Some((id, username)) = resetting() else {
            return;
        };
        spawn(async move {
            let request = SetPasswordRequest {
                password: new_password(),
            };
            match api::send_empty(
                "POST",
                &format!("/api/v1/users/{id}/password"),
                Some(&request),
            )
            .await
            {
                Ok(()) => {
                    notice.set(Some(format!(
                        "password reset for {username}; sessions signed out"
                    )));
                    resetting.set(None);
                    new_password.set(String::new());
                }
                Err(error) => notice.set(Some(error.to_string())),
            }
        });
    };

    rsx! {
        div { class: "page-head",
            h1 { "Users" }
            div { class: "head-actions",
                Link { class: "btn btn-primary", to: Route::UsersCreate {}, "New user" }
            }
        }
        if let Some(message) = notice() {
            div { class: "card notice", "{message}" }
        }
        if let Some((_, username)) = resetting() {
            div { class: "selection-bar",
                span { "New password for {username}" }
                input {
                    class: "input",
                    r#type: "password",
                    autocomplete: "new-password",
                    value: "{new_password}",
                    oninput: move |event| new_password.set(event.value()),
                }
                span { class: "spacer" }
                button { class: "btn btn-sm btn-primary", onclick: confirm_reset, "Reset" }
                button { class: "btn btn-sm", onclick: move |_| resetting.set(None), "Cancel" }
            }
        }
        match users() {
            None => rsx! { Loading {} },
            Some(Err(error)) => rsx! { ErrorState { err: error, what: "accounts".to_owned() } },
            Some(Ok(list)) => {
                let rows = filtered(&list, &query(), sort());
                let shown = page_slice(&rows, page(), per_page());
                rsx! {
                    div { class: "table-controls",
                        SearchBox { query, page, placeholder: "Search accounts…" }
                    }
                    div { class: "table-wrap",
                        table { class: "table",
                            thead { tr {
                                SortableTh { label: "username", sort_key: "username", sort }
                                SortableTh { label: "source", sort_key: "source", sort }
                                SortableTh { label: "created", sort_key: "created", sort }
                                th { "" }
                            } }
                            tbody {
                                if shown.is_empty() {
                                    tr { td { colspan: 4, class: "muted center", "No accounts match." } }
                                }
                                for user in shown.iter() {
                                    tr {
                                        td { "{user.username}" }
                                        td { "{user.auth_source}" }
                                        td { class: "muted", "{human_time(user.created_at_unix, now_secs())}" }
                                        td { class: "row-actions",
                                            if user.auth_source == "authdb" {
                                                button {
                                                    class: "btn btn-sm",
                                                    onclick: {
                                                        let target = (user.id, user.username.clone());
                                                        move |_| resetting.set(Some(target.clone()))
                                                    },
                                                    "Reset password"
                                                }
                                            }
                                            if Some(user.id) != own_id {
                                                button {
                                                    class: "btn btn-sm btn-danger",
                                                    onclick: {
                                                        let id = user.id;
                                                        move |_| delete(id)
                                                    },
                                                    "Delete"
                                                }
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                    Pager { page, per_page, total_rows: rows.len(), noun: "accounts" }
                }
            }
        }
    }
}

fn filtered(all: &[WebuiUserInfo], query: &str, (key, ascending): Sort) -> Vec<WebuiUserInfo> {
    let mut rows: Vec<WebuiUserInfo> = all
        .iter()
        .filter(|row| row_matches(query, &[&row.username, &row.auth_source]))
        .cloned()
        .collect();
    rows.sort_by(|left, right| {
        let ordering = match key {
            "source" => left.auth_source.cmp(&right.auth_source),
            "created" => left.created_at_unix.cmp(&right.created_at_unix),
            _ => left.username.cmp(&right.username),
        };
        if ascending {
            ordering
        } else {
            ordering.reverse()
        }
    });
    rows
}
