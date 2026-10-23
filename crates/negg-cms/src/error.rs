// SPDX-License-Identifier: MPL-2.0

//! The error type for colour management.

use std::fmt;

/// Errors raised by `negg-cms`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CmsError {
    /// Little-CMS refused an operation. The string names the operation and
    /// the error Little-CMS reported.
    Lcms(String),
    /// A 3x3 matrix has no inverse (its determinant is zero, or the
    /// primaries it was built from are degenerate).
    Singular,
    /// The input is outside what this crate handles; the string says what.
    Unsupported(String),
}

impl fmt::Display for CmsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            CmsError::Lcms(what) => write!(f, "Little-CMS error: {what}"),
            CmsError::Singular => f.write_str("matrix is singular"),
            CmsError::Unsupported(what) => write!(f, "unsupported: {what}"),
        }
    }
}

impl std::error::Error for CmsError {}

/// Wraps a Little-CMS error with the operation it came from.
pub(crate) fn lcms_error(operation: &str, error: lcms2::Error) -> CmsError {
    CmsError::Lcms(format!("{operation}: {error:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_names_the_problem() {
        assert_eq!(
            CmsError::Lcms("opening profile: MissingData".into()).to_string(),
            "Little-CMS error: opening profile: MissingData"
        );
        assert_eq!(CmsError::Singular.to_string(), "matrix is singular");
        assert_eq!(
            CmsError::Unsupported("CMYK".into()).to_string(),
            "unsupported: CMYK"
        );
    }

    #[test]
    fn lcms_error_carries_the_operation_and_the_variant() {
        let e = lcms_error("building the transform", lcms2::Error::ObjectCreationError);
        assert_eq!(
            e,
            CmsError::Lcms("building the transform: ObjectCreationError".into())
        );
    }
}
