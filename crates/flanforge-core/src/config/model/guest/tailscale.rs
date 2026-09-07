use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct TailscaleConfig {
    pub enabled: bool,
    pub preauth_key_file: Option<PathBuf>,
    pub login_server: Option<Url>,
    pub hostname: Option<String>,
    pub extra_args: String,
}
