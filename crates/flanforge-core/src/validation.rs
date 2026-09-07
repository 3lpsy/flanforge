use std::borrow::Cow;

use validator::{ValidationErrors, ValidationErrorsKind};

/// Names one failing field and its validator code, for a log line.
///
/// Reads field names and validator codes only, never `params`: the derive puts
/// the field's own value in `params["value"]` and `ValidationErrors`' `Display`
/// prints it, so formatting the errors would echo the rejected input.
///
/// A record failing several fields reports *a* failing field, chosen by name so
/// the same input always logs the same line. Only static names survive: an
/// owned one came from somewhere other than a literal in this workspace.
#[must_use]
pub fn first_field_error(errors: &ValidationErrors) -> (&'static str, &'static str) {
    let mut named: Vec<(&'static str, &'static str)> = errors
        .errors()
        .iter()
        .map(|(field, kind)| {
            let field = match field {
                Cow::Borrowed(field) => *field,
                Cow::Owned(_) => "unknown",
            };
            (field, kind_code(kind))
        })
        .collect();
    named.sort_unstable();
    named.first().copied().unwrap_or(("unknown", "unknown"))
}

fn kind_code(kind: &ValidationErrorsKind) -> &'static str {
    match kind {
        ValidationErrorsKind::Field(errors) => match errors.first().map(|error| &error.code) {
            Some(Cow::Borrowed(code)) => code,
            _ => "unknown",
        },
        ValidationErrorsKind::Struct(_) | ValidationErrorsKind::List(_) => "nested",
    }
}
