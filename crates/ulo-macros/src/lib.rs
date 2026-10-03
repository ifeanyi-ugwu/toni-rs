//! Macros for `ulo`. Each expands to calls on the core's value-level API, which integration
//! crates call directly; nothing here does what that API cannot.
//!
//! Errors are `syn::Error`s spanned on the offending token, and per-field and per-parameter
//! assertions are emitted with `quote_spanned!`, so a compile error points at the field,
//! parameter or attribute rather than at the macro invocation.

mod construct_attr;
mod enhancers;
mod injectable;
mod module_attr;
mod routes;
mod shared;

use proc_macro::TokenStream;

/// A type the container builds.
///
/// On a struct, every field is an injection point except one marked `#[injectable(default)]`,
/// which is set from `Default`. On an impl block, the constructor is the fn named `new`, or the
/// one marked `#[construct]`; its parameters are injection points, it may be `async`, and it may
/// return `Self` or a `Result<Self, E>`.
///
/// Arguments: a scope, `singleton`, `execution` or `transient` (none means `Auto`), and
/// `timeout = <Duration expr>`, which writes `CONSTRUCT_TIMEOUT`. `Construct::hooks` is filled by
/// probing the five lifecycle traits.
#[proc_macro_attribute]
pub fn injectable(attr: TokenStream, item: TokenStream) -> TokenStream {
    injectable::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Marks the constructor inside an `#[injectable]` impl when it is not named `new`. Anywhere
/// else it is an error.
#[proc_macro_attribute]
pub fn construct(attr: TokenStream, item: TokenStream) -> TokenStream {
    construct_attr::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A module: `imports`, `providers`, `controllers`, `exports` and `global`, lowered to a
/// `Module` impl whose `register` calls the value API. A struct with fields is a configured
/// module, identified by value, and every `Secret<_>` field is registered for redaction.
///
/// Provider entries: `Type` (`provide::<Type>()`), `Type as dyn Trait` (`also_as`), `expr?`
/// (`try_value`), `expr` (`value`), a factory closure (`singleton` or `try_singleton`, picked by
/// the closure's output), and `into dyn Trait: [..]` (contributions). A bare path reads as a
/// type, so a constant is bound by value with a block, `{ LIMITS }`. Export entries: `Type` and
/// `reexport Type`.
///
/// An `into K: [..]` list takes the forms `#[guards]` does, whatever `K` is: a type
/// (`provide::<A>`), `value = expr` (a shared value; the macro writes the `Arc::new`) and
/// `with = |..| ..` (a singleton factory, written as in `#[guards]`, `try_singleton` when its
/// output is a `Result`). Every item lowers to `contribute::<K>()`. A global enhancer is a
/// contribution under a role key, `into AnyGuard<Http>: [AuthGuard]`, which the core recognises
/// by the key's type.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_attr::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A controller's handler impl: writes `impl Controller` whose `mount` registers each handler,
/// with the impl's `#[guards]`, `#[interceptors]` and `#[error_handlers]` as the controller tier
/// and each method's as the method tier; every other item stays as written.
///
/// A method carrying an attribute outside the language's own (`doc`, `allow`, `cfg`, `inline` and
/// the like), such as `#[tracing::instrument]`, is treated as a handler, so helpers go in a
/// separate `impl` block.
#[proc_macro_attribute]
pub fn routes(attr: TokenStream, item: TokenStream) -> TokenStream {
    routes::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Guards for every handler of a `#[routes]` impl, or for one method: by type (`AuthGuard`),
/// transport-scoped by type (`http = AuthGuard`), by value (`value = expr`), or by closure
/// (`with = |u: Ext<CurrentUser>| ..`). Read by `#[routes]`, so on the impl it goes below
/// `#[routes]`; anywhere else it is an error.
///
/// On the impl, an entry applies to every handler and must have the role for each handler's
/// transport; `http = AuthGuard` limits it to that transport's handlers. `value = expr` is
/// written per method, where it is built once and shared by that handler's calls. On the impl it
/// is a compile error: a guard whose state every handler shares is declared by type, as a
/// singleton binding. A closure is written synchronously and built per execution, its parameters
/// injection points.
#[proc_macro_attribute]
pub fn guards(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("guards", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Interceptors, with the same forms as `#[guards]`.
#[proc_macro_attribute]
pub fn interceptors(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("interceptors", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Error handlers, with the same forms as `#[guards]`.
#[proc_macro_attribute]
pub fn error_handlers(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::marker("error_handlers", attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Carries a handler's controller-tier and method-tier enhancers from `#[routes]` to the
/// transport's handler attribute, which consumes it. Reaching expansion means no transport
/// attribute consumed it.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::unconsumed_handler(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Invoked by a transport's generated mount code with its transport type, its scope key and the
/// tokens `__handler` carried: expands to the two `EnhancerSpec`s for one handler, with one role
/// assertion per enhancer naming the handler.
#[doc(hidden)]
#[proc_macro]
pub fn __enhancer_specs(input: TokenStream) -> TokenStream {
    enhancers::specs(input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
