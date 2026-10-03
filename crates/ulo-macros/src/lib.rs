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
/// (`try_value`), `expr` (`value`), `with = |..| ..` (`with`, or `try_with` when the closure's
/// output is a `Result`), `with(<scope>) = |..| ..` (`singleton`, `execution` or `transient`, or
/// the `try_` form), a bare closure (shorthand for `with = closure`), `K: with = |..| ..` and
/// `K: with(<scope>) = |..| ..` (the same binding, also bound under the trait key `K` with
/// `also_as::<K>(|a| a)`), and `into dyn Trait: [..]` (contributions). A bare path reads as a
/// type, so a constant is bound by value with a block, `{ LIMITS }`. Export entries: `Type` and
/// `reexport Type`.
///
/// The key comes first in `K: with = ..` because a closure's body extends as far as it can: in
/// `with = |cfg| RedisCache::new(cfg) as dyn Cache`, the `as` would parse as a cast inside the
/// body. As with `X as dyn T`, the concrete type stays bound in the module too.
///
/// An `into K: [..]` list takes the forms `#[guards]` does, whatever `K` is: a type
/// (`provide::<A>`), `value = expr` (a shared value; the macro writes the `Arc::new`),
/// `with = |..| ..` (a factory written as in `#[guards]`: `with`, or `try_with` when its output
/// is a `Result`) and `with(<scope>) = |..| ..` (`singleton`, `execution` or `transient`, or the
/// `try_` form of each). It also takes `value = expr?` (`try_value`), whose `Err` `wire()`
/// reports as it reports a providers entry's `expr?`. Every item lowers to `contribute::<K>()`.
/// A global enhancer is a contribution under a role key, `into AnyGuard<Http>: [AuthGuard]`,
/// which the core recognises by the key's type.
///
/// `with` says how an item is built: by its closure. How long it lives is a separate argument,
/// written as on `#[injectable]`. `with = ..` declares `Auto`: built once unless what the closure
/// reads needs an execution, then built per execution. A provider contribution under `Auto` is a
/// singleton, and `wire()` refuses one whose closure reads execution data. `with(singleton) = ..`,
/// `with(execution) = ..` and `with(transient) = ..` write the scope instead.
///
/// Inference reads what a closure takes, not what it does. A closure that creates per-call state
/// but reads nothing per call, such as `|| RequestTimer::start()`, is built once under `Auto`;
/// it must be written `with(execution) = || RequestTimer::start()`.
#[proc_macro_attribute]
pub fn module(attr: TokenStream, item: TokenStream) -> TokenStream {
    module_attr::expand(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// A controller's handler impl: writes `impl Controller` whose `mount` registers each handler,
/// with the impl's `#[guards]`, `#[interceptors]`, `#[error_handlers]` and `#[meta]` as the
/// controller tier and each method's as the method tier; every other item stays as written.
///
/// An impl-level `value = expr` is built once by `Controller::mount` and shared by every handler
/// it applies to, so a rate limiter declared on the impl limits the controller as a whole. A
/// controller-level transport key that no handler's transport carries, `#[guards(htpp = ..)]`, is
/// a compile error spanned on the key.
///
/// A method carrying an attribute outside the language's own (`doc`, `allow`, `cfg`, `inline` and
/// the like) and the enhancer and `#[meta]` markers, such as `#[tracing::instrument]`, is treated
/// as a handler, so helpers go in a separate `impl` block.
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
/// transport; `http = AuthGuard` limits it to that transport's handlers. `value = expr` is built
/// once and shared: on a method by that handler's calls, on the impl by every handler it applies
/// to.
///
/// A closure is written synchronously, its parameters injection points. `with` says only that
/// the closure builds the guard; how long the guard lives is a separate argument, written as on
/// `#[injectable]`. `with = ..` declares `Auto`: built once and shared unless what the closure
/// reads needs an execution, then built per execution. `with(singleton) = ..`,
/// `with(execution) = ..` and `with(transient) = ..` write the scope instead, and
/// `http(with(execution) = ..)` limits one to a transport's handlers.
///
/// Inference reads what a closure takes, not what it does. A closure that creates per-call state
/// but reads nothing per call, such as `|| RequestTimer::start()`, is built once under `Auto`;
/// it must be written `with(execution) = || RequestTimer::start()`.
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

/// Declared metadata on a handler or on its `#[routes]` impl: plain `Send + Sync + 'static`
/// values, built once when the handler mounts, which guards and interceptors read through
/// `cx.exec().handler()`. The method's declaration of a type wins over the impl's. Read by
/// `#[routes]`, so on the impl it goes below `#[routes]`; anywhere else it is an error.
#[proc_macro_attribute]
pub fn meta(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::meta_marker(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Carries a handler's controller-tier and method-tier enhancers and metadata from `#[routes]` to
/// the transport's handler attribute, which consumes it (`ulo_handler_codegen::protocol`).
/// Reaching expansion means no transport attribute consumed it.
#[doc(hidden)]
#[proc_macro_attribute]
pub fn __handler(attr: TokenStream, item: TokenStream) -> TokenStream {
    enhancers::unconsumed_handler(attr.into(), item.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}

/// Invoked by a transport's generated mount function with its transport type, its transport key,
/// the name of its `shared` parameter and the tokens `__handler` carried: expands to the two
/// `EnhancerSpec`s for one handler, with one role assertion per enhancer naming the handler.
#[doc(hidden)]
#[proc_macro]
pub fn __enhancer_specs(input: TokenStream) -> TokenStream {
    enhancers::specs(input.into()).unwrap_or_else(syn::Error::into_compile_error).into()
}
