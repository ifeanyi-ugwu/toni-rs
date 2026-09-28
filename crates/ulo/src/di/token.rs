//! The one derivation of a container key from a type.

/// The canonical DI token for a type: its fully-qualified `type_name`, base and
/// generic parameters alike.
///
/// This is the one definition of a type-derived token. Every site that turns a
/// type into a container key — macro-generated registration and injection,
/// `resolve`, exports, library provider factories — goes through here, so the
/// two sides of a lookup can only agree. `type_name` output is not stable
/// across compiler versions, but a token never leaves the process: the same
/// binary computes both sides, which is all the equality needs.
pub fn token_of<T: ?Sized>() -> String {
    std::any::type_name::<T>().to_string()
}
