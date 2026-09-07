#[derive(Clone, Default)]
pub struct ConfigOverrides {
    entries: Vec<(String, String)>,
}

impl ConfigOverrides {
    #[must_use]
    pub fn new<'a>(entries: impl IntoIterator<Item = (&'a str, &'a str)>) -> Self {
        Self {
            entries: entries
                .into_iter()
                .map(|(key, value)| (key.to_owned(), value.to_owned()))
                .collect(),
        }
    }

    pub(crate) fn pairs(&self) -> Vec<(&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
            .collect()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, &str)> {
        self.entries
            .iter()
            .map(|(key, value)| (key.as_str(), value.as_str()))
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

impl std::fmt::Debug for ConfigOverrides {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ConfigOverrides")
            .field(
                "keys",
                &self.entries.iter().map(|(key, _)| key).collect::<Vec<_>>(),
            )
            .finish()
    }
}
