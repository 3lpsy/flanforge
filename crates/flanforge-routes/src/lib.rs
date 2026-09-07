mod auth;
mod error;
mod handlers;
mod input;
mod operator;

pub use auth::{Authenticated, ensure_authenticated};
pub use handlers::{AppState, cancel, create, health, status};
pub use input::ValidatedJson;
pub use operator::{
    OperatorState, cancel as operator_cancel, ensure_local_peer, hot as operator_hot,
    hot_retire as operator_hot_retire, list as operator_list, reap as operator_reap,
    status as operator_status, webui_user_create as operator_webui_user_create,
    webui_user_delete as operator_webui_user_delete,
    webui_user_reset_password as operator_webui_user_reset_password,
    webui_users_list as operator_webui_users_list,
};

#[cfg(test)]
mod tests;
