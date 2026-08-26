//! DI tokens: the canonical type-derived key, named const tokens, and the
//! conversion trait the lookup APIs accept.

use std::marker::PhantomData;

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

/// A named token for identifying providers in the DI container.
///
/// The type parameter documents what the token resolves to; retrieval is still
/// by name, so the parameter is not enforced at the lookup site.
///
/// # Examples
///
/// ```rust,ignore
/// use toni::di::Token;
///
/// pub const MY_SERVICE: Token<MyService> = Token::new("MY_SERVICE");
/// ```
pub struct Token<T: ?Sized> {
    name: &'static str,
    _phantom: PhantomData<fn() -> T>,
}

impl<T: ?Sized> Token<T> {
    /// Creates a new token with the given name
    ///
    /// This is a const function, so tokens can be defined as constants.
    pub const fn new(name: &'static str) -> Self {
        Self {
            name,
            _phantom: PhantomData,
        }
    }

    /// Returns the string name of this token
    pub const fn name(&self) -> &'static str {
        self.name
    }
}

// Implement Clone, Copy, Debug, PartialEq, Eq manually since PhantomData is always these
impl<T: ?Sized> Clone for Token<T> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<T: ?Sized> Copy for Token<T> {}

impl<T: ?Sized> std::fmt::Debug for Token<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Token").field("name", &self.name).finish()
    }
}

impl<T: ?Sized> PartialEq for Token<T> {
    fn eq(&self, other: &Self) -> bool {
        self.name == other.name
    }
}

impl<T: ?Sized> Eq for Token<T> {}

impl<T: ?Sized> std::hash::Hash for Token<T> {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.name.hash(state);
    }
}

/// Conversion into a DI container key, accepted by the by-token lookup APIs
/// (`get_by_token`, `resolve_by_token`).
pub trait IntoToken {
    fn into_token(self) -> String;
}

impl IntoToken for &str {
    fn into_token(self) -> String {
        self.to_string()
    }
}

impl IntoToken for String {
    fn into_token(self) -> String {
        self
    }
}

impl<T: ?Sized> IntoToken for Token<T> {
    fn into_token(self) -> String {
        self.name().to_string()
    }
}

// Tokens the scanner recognizes as global-enhancer registrations.

// Guard token for global guards
// Usage: container.add_provider(APP_GUARD, MyGlobalGuard)
pub const APP_GUARD: Token<()> = Token::new("__TONI_APP_GUARD__");

// Interceptor token for global interceptors
// Usage: container.add_provider(APP_INTERCEPTOR, MyGlobalInterceptor)
pub const APP_INTERCEPTOR: Token<()> = Token::new("__TONI_APP_INTERCEPTOR__");

// Middleware token for global middleware
// Usage: container.add_provider(APP_MIDDLEWARE, MyGlobalMiddleware)
pub const APP_MIDDLEWARE: Token<()> = Token::new("__TONI_APP_MIDDLEWARE__");
