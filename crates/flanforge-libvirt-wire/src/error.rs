#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum WireError {
    #[error("{contract} cannot be encoded")]
    Encode { contract: &'static str },
    #[error("{contract} cannot be decoded")]
    Decode { contract: &'static str },
    #[error("{contract} field {field} is structurally invalid")]
    Invalid {
        contract: &'static str,
        field: &'static str,
    },
}

impl WireError {
    pub(crate) const fn encode(contract: &'static str) -> Self {
        Self::Encode { contract }
    }

    pub(crate) const fn decode(contract: &'static str) -> Self {
        Self::Decode { contract }
    }

    pub(crate) const fn invalid(contract: &'static str, field: &'static str) -> Self {
        Self::Invalid { contract, field }
    }
}
