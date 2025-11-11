//! Type-safe DI tokens for identifying providers
//!
//! Tokens are used to identify providers in the DI container, especially for
//! built-in framework providers like guards, interceptors, pipes, and middleware.

use std::marker::PhantomData;

/// A type-safe token for identifying providers in the DI container
///
/// The token carries both a string name and a phantom type parameter to ensure
/// type safety when retrieving providers.
///
/// # Examples
///
/// ```rust,ignore
/// use toni::di::Token;
/// use toni::traits_helpers::Guard;
///
/// // Framework-provided token for guards
/// pub const APP_GUARD: Token<dyn Guard> = Token::new("__TONI_APP_GUARD__");
///
/// // Custom token
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
    pub fn name(&self) -> &'static str {
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

// Framework constants - these will be used for global enhancers

// Guard token for global guards
// Usage: container.add_provider(APP_GUARD, MyGlobalGuard)
pub const APP_GUARD: Token<()> = Token::new("__TONI_APP_GUARD__");

// Interceptor token for global interceptors
// Usage: container.add_provider(APP_INTERCEPTOR, MyGlobalInterceptor)
pub const APP_INTERCEPTOR: Token<()> = Token::new("__TONI_APP_INTERCEPTOR__");

// Pipe token for global pipes
// Usage: container.add_provider(APP_PIPE, MyGlobalPipe)
pub const APP_PIPE: Token<()> = Token::new("__TONI_APP_PIPE__");

// Middleware token for global middleware
// Usage: container.add_provider(APP_MIDDLEWARE, MyGlobalMiddleware)
pub const APP_MIDDLEWARE: Token<()> = Token::new("__TONI_APP_MIDDLEWARE__");
