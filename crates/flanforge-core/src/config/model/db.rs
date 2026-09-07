use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// File name used when `db.db_path` is not set.
pub const DEFAULT_DB_FILE_NAME: &str = "flanforge.db";

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct DbConfig {
    pub db_path: Option<PathBuf>,
}

impl DbConfig {
    /// The database path to open: `db_path` when set, otherwise
    /// `flanforge.db` beside the configuration file itself.
    #[must_use]
    pub fn resolved_db_path(&self, config_path: &Path) -> PathBuf {
        self.db_path.clone().unwrap_or_else(|| {
            config_path.parent().map_or_else(
                || PathBuf::from(DEFAULT_DB_FILE_NAME),
                |directory| directory.join(DEFAULT_DB_FILE_NAME),
            )
        })
    }
}
