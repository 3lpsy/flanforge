use std::str::FromStr;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConfigOverrideArgument {
    key: String,
    value: String,
}

impl ConfigOverrideArgument {
    #[must_use]
    pub fn key(&self) -> &str {
        &self.key
    }

    #[must_use]
    pub fn value(&self) -> &str {
        &self.value
    }
}

impl FromStr for ConfigOverrideArgument {
    type Err = String;

    fn from_str(input: &str) -> Result<Self, Self::Err> {
        let (key, value) = input
            .split_once('=')
            .ok_or_else(|| "configuration override must be KEY=VALUE".to_owned())?;
        if key.is_empty()
            || key.len() > 192
            || key.split('.').any(|segment| {
                segment.is_empty()
                    || segment.len() > 64
                    || !segment
                        .bytes()
                        .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
            })
        {
            return Err("configuration override key is invalid".to_owned());
        }
        if value.len() > 65_536
            || value
                .bytes()
                .any(|byte| byte == 0 || byte.is_ascii_control())
        {
            return Err("configuration override value is invalid".to_owned());
        }
        Ok(Self {
            key: key.to_ascii_lowercase(),
            value: value.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn override_arguments_validate_structure_without_interpreting_values() {
        let parsed = "runtime.backend.qemu_img_path=/opt/bin/qemu-img"
            .parse::<ConfigOverrideArgument>()
            .unwrap_or_else(|error| unreachable!("valid override: {error}"));
        assert_eq!(parsed.key(), "runtime.backend.qemu_img_path");
        assert_eq!(parsed.value(), "/opt/bin/qemu-img");

        for rejected in [
            "runtime.backend.kind",
            "runtime..kind=libvirt",
            "runtime.backend.kind=libvirt\nother",
        ] {
            assert!(
                rejected.parse::<ConfigOverrideArgument>().is_err(),
                "{rejected}"
            );
        }
        let empty = "tailscale.extra_args="
            .parse::<ConfigOverrideArgument>()
            .unwrap_or_else(|error| unreachable!("empty string override: {error}"));
        assert_eq!(empty.value(), "");
    }
}
