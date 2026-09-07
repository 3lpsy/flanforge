use jsonwebtoken::errors::ErrorKind;
use thiserror::Error;

/// Claim names this daemon requires. A library-reported missing claim is
/// matched against this list so the stored name is always one of our own
/// literals, never an attacker-influenced string.
const REQUIRED_CLAIMS: [&str; 6] = ["aud", "exp", "iat", "iss", "nbf", "sub"];

/// Why authentication failed.
///
/// Every payload is `&'static str` or a scalar, which keeps the type `Copy` and
/// makes each reason assertable in a unit test rather than only observable in a
/// captured log. The `Display` strings are deliberately coarse: they are the
/// operator-facing summary, and the discriminating detail travels in the
/// payload, which only the log site reads.
#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum AuthError {
    #[error("authentication credentials are required")]
    MissingCredentials,
    #[error("authentication token is invalid")]
    InvalidToken(TokenRejection),
    #[error("identity provider is unavailable")]
    Unavailable(ProviderFailure),
}

/// Which token check refused. Nothing here carries a claim *value*; the two
/// variants that name anything name one of our own claim or field names.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TokenRejection {
    /// Credential presentation, before any signature work.
    DuplicateHeader,
    NonUtf8Header,
    Scheme,
    CompactShape,
    Oversize,
    /// Header and key selection, still before signature verification.
    Header,
    Algorithm,
    KeyId,
    UnknownKey,
    AmbiguousKey,
    KeyUnusable,
    /// Signature and registered-claim verification.
    Signature,
    Issuer,
    Audience,
    Expired,
    NotYetValid,
    MissingClaim(&'static str),
    Payload,
    /// Post-signature checks on this daemon's own claim policy.
    ClaimShape {
        field: &'static str,
        code: &'static str,
    },
    IssuedInFuture,
    LifetimeCap,
    Clock,
}

impl TokenRejection {
    /// Maps a library rejection onto a stable local reason.
    #[must_use]
    pub fn from_jwt(kind: &ErrorKind) -> Self {
        match kind {
            ErrorKind::InvalidSignature | ErrorKind::Crypto(_) => Self::Signature,
            ErrorKind::InvalidIssuer => Self::Issuer,
            ErrorKind::InvalidAudience => Self::Audience,
            ErrorKind::ExpiredSignature => Self::Expired,
            ErrorKind::ImmatureSignature => Self::NotYetValid,
            ErrorKind::InvalidAlgorithm | ErrorKind::InvalidAlgorithmName => Self::Algorithm,
            ErrorKind::MissingRequiredClaim(name) => Self::MissingClaim(
                REQUIRED_CLAIMS
                    .into_iter()
                    .find(|claim| claim == name)
                    .unwrap_or("unknown"),
            ),
            ErrorKind::Json(_) | ErrorKind::Base64(_) | ErrorKind::Utf8(_) => Self::Payload,
            _ => Self::Header,
        }
    }

    /// A stable log code. Never interpolated into a response body.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::DuplicateHeader => "duplicate_header",
            Self::NonUtf8Header => "non_utf8_header",
            Self::Scheme => "scheme",
            Self::CompactShape => "compact_shape",
            Self::Oversize => "oversize",
            Self::Header => "header",
            Self::Algorithm => "algorithm",
            Self::KeyId => "key_id",
            Self::UnknownKey => "unknown_key",
            Self::AmbiguousKey => "ambiguous_key",
            Self::KeyUnusable => "key_unusable",
            Self::Signature => "signature",
            Self::Issuer => "issuer",
            Self::Audience => "audience",
            Self::Expired => "expired",
            Self::NotYetValid => "not_yet_valid",
            Self::MissingClaim(_) => "missing_claim",
            Self::Payload => "payload",
            Self::ClaimShape { .. } => "claim_shape",
            Self::IssuedInFuture => "issued_in_future",
            Self::LifetimeCap => "lifetime_cap",
            Self::Clock => "clock",
        }
    }

    /// The claim or field the reason is about, or `-` when it is about none.
    #[must_use]
    pub fn field(self) -> &'static str {
        match self {
            Self::MissingClaim(claim) => claim,
            Self::ClaimShape { field, .. } => field,
            _ => "-",
        }
    }

    /// The validator code behind a claim-shape rejection, or `-`.
    #[must_use]
    pub fn detail(self) -> &'static str {
        match self {
            Self::ClaimShape { code, .. } => code,
            _ => "-",
        }
    }
}

/// Why the identity provider could not be consulted. Separates "JWKS host is
/// down" from "JWKS answered with nothing usable", which the single
/// `Unavailable` variant could not.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProviderFailure {
    Client,
    Connect,
    Status,
    Oversize,
    Malformed,
    EmptyKeySet,
    Cooldown,
}

impl ProviderFailure {
    /// A stable log code. Never interpolated into a response body.
    #[must_use]
    pub fn code(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Connect => "connect",
            Self::Status => "status",
            Self::Oversize => "oversize",
            Self::Malformed => "malformed",
            Self::EmptyKeySet => "empty_key_set",
            Self::Cooldown => "cooldown",
        }
    }
}
