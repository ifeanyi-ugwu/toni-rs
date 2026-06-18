//! `#[derive(toni::Error)]` exercises the codegen across struct, enum, and
//! default-fallback shapes.

use std::fmt;
use toni::{Error, ErrorKind};

#[derive(toni::Error)]
#[error_kind(NotFound)]
struct StructTagged(String);

impl fmt::Debug for StructTagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StructTagged({})", self.0)
    }
}

impl fmt::Display for StructTagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StructTagged {}

#[derive(toni::Error)]
struct StructUntagged(String);

impl fmt::Debug for StructUntagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "StructUntagged({})", self.0)
    }
}

impl fmt::Display for StructUntagged {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for StructUntagged {}

#[derive(Debug, toni::Error)]
enum BillingError {
    #[error_kind(NotFound)]
    InvoiceMissing(String),

    #[error_kind(UnprocessableEntity)]
    CardDeclined,

    #[error_kind(Unavailable)]
    ServiceDown {
        retry_after: u32,
    },

    // Untagged → no enum-level default → falls all the way through to Internal.
    Other,
}

impl fmt::Display for BillingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvoiceMissing(id) => write!(f, "invoice {id} missing"),
            Self::CardDeclined => f.write_str("card declined"),
            Self::ServiceDown { retry_after } => write!(f, "service down (retry {retry_after}s)"),
            Self::Other => f.write_str("other"),
        }
    }
}

impl std::error::Error for BillingError {}

#[derive(Debug, toni::Error)]
#[error_kind(Forbidden)]
enum AuthError {
    BadToken,
    #[error_kind(Unauthorized)]
    Missing,
}

impl fmt::Display for AuthError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::BadToken => f.write_str("bad token"),
            Self::Missing => f.write_str("missing"),
        }
    }
}

impl std::error::Error for AuthError {}

#[test]
fn struct_tag_drives_kind() {
    assert_eq!(StructTagged("x".into()).kind(), ErrorKind::NotFound);
}

#[test]
fn untagged_struct_defaults_to_internal() {
    assert_eq!(StructUntagged("x".into()).kind(), ErrorKind::Internal);
}

#[test]
fn enum_per_variant_tag_wins() {
    assert_eq!(
        BillingError::InvoiceMissing("inv-1".into()).kind(),
        ErrorKind::NotFound,
    );
    assert_eq!(
        BillingError::CardDeclined.kind(),
        ErrorKind::UnprocessableEntity,
    );
    assert_eq!(
        BillingError::ServiceDown { retry_after: 30 }.kind(),
        ErrorKind::Unavailable,
    );
}

#[test]
fn untagged_variant_uses_enum_default() {
    // No top-level default on BillingError → falls to Internal.
    assert_eq!(BillingError::Other.kind(), ErrorKind::Internal);
    // AuthError's enum-level default applies to its untagged variant.
    assert_eq!(AuthError::BadToken.kind(), ErrorKind::Forbidden);
    // Per-variant tag still overrides the enum-level default.
    assert_eq!(AuthError::Missing.kind(), ErrorKind::Unauthorized);
}

#[test]
fn default_message_uses_display() {
    assert_eq!(BillingError::CardDeclined.message(), "card declined");
}
