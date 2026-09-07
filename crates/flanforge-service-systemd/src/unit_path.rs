use std::path::{Component, Path};

use anyhow::{Result, bail};

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct UnitPath(String);

impl UnitPath {
    pub(crate) fn new(path: &Path) -> Result<Self> {
        let Some(value) = path.to_str() else {
            bail!("systemd path is not UTF-8");
        };
        if !path.is_absolute()
            || path
                .components()
                .any(|component| matches!(component, Component::ParentDir))
            || value.is_empty()
            || value.len() > 1_024
            || value
                .bytes()
                .any(|byte| byte.is_ascii_control() || matches!(byte, b'"' | b'\\'))
        {
            bail!("systemd path is not a safe absolute path");
        }
        Ok(Self(value.to_owned()))
    }

    pub(crate) fn render(&self) -> String {
        format!("\"{}\"", self.0.replace('%', "%%").replace('$', "$$"))
    }
}
