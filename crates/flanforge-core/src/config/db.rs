use super::{ConfigError, DbConfig, validate::ensure_path};

pub(super) fn ensure_db_valid(db: &DbConfig) -> Result<(), ConfigError> {
    if let Some(path) = &db.db_path {
        ensure_path("db.db_path", path)?;
    }
    Ok(())
}
