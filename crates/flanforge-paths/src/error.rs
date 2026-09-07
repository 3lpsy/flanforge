use std::fmt;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PathError {
    HomeUnavailable,
    HomeNotAbsolute,
    PathNotAbsolute,
    PathNotNormalized,
    UnsafeName,
}

impl fmt::Display for PathError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::HomeUnavailable => "HOME is required",
            Self::HomeNotAbsolute => "HOME must be an absolute path",
            Self::PathNotAbsolute => "path must be absolute",
            Self::PathNotNormalized => "path must not contain current or parent traversal",
            Self::UnsafeName => "path component name is unsafe",
        })
    }
}

impl std::error::Error for PathError {}
