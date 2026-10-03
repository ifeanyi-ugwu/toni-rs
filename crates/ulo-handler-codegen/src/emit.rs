//! The items a transport attribute owes `#[routes]` for one handler (see [`crate::protocol`]).
//!
//! For `#[ulo_http::get("/users/{id}")] async fn get(&self, id: Path<u64>, user: Ext<CurrentUser>)
//! -> Result<Json<User>, UserError>` inside a `#[routes]` impl with one impl-level
//! `#[interceptors(value = Timing::new())]`, the expansion is, in shape:
//!
//! ```text
//! async fn get(&self, id: Path<u64>, user: Ext<CurrentUser>) -> Result<Json<User>, UserError> { .. }
//!
//! #[doc(hidden)] #[allow(non_upper_case_globals)]
//! const __ULO_KEY_get: &'static str = <::ulo_http::Http as ::ulo::Transport>::KEY;
//!
//! #[doc(hidden)] #[allow(non_upper_case_globals)]
//! const __ULO_CHECKS_get: () = { assert!(!(<Path<u64> as P<Http, _>>::CONSUMES_BODY && ..), "..") };
//!
//! #[doc(hidden)]
//! fn __ulo_mount_get<__UloV0: ::ulo::Interceptor<::ulo_http::Http>>(
//!     m: &mut ::ulo::Mount<'_>,
//!     __ulo_shared: &::ulo::__private::Shared<(::ulo::__private::Arc<__UloV0>,)>,
//! ) {
//!     let (controller, method) = ::ulo::__private::__enhancer_specs!(::ulo_http::Http, "http", __ulo_shared, <tokens>);
//!     let mut meta = ::ulo::Metadata::new(); /* meta.controller(..); meta.method(..); */
//!     let mut dependencies = ::ulo::Dependencies::default();
//!     <Path<u64> as P<Http, _>>::dependencies(&mut dependencies); /* one per parameter */
//!     let handler = <transport's handler value, its call closure built from `call_body`>;
//!     m.handler(::ulo::HandlerSpec::new("get", handler)
//!         .controller(controller).method(method).meta(meta).route(..).shape(..).dependencies(dependencies));
//! }
//! ```
//!
//! `P` is `<transport>::__private::Param`. The call closure's body, from [`call_body`]:
//!
//! ```text
//! let __ulo_arg0: Path<u64> = <Path<u64> as P<Http, _>>::extract(&cx).await?;
//! let __ulo_arg1: Ext<CurrentUser> = <Ext<CurrentUser> as P<Http, _>>::extract(&cx).await?;
//! let __ulo_this = <transport>::__private::controller::<Self, Http>(&cx).await?;
//! let __ulo_out = Self::get(&*__ulo_this, __ulo_arg0, __ulo_arg1).await;
//! (&&&<transport>::__private::IntoReplyProbe::<Http, _>::new(__ulo_out)).into_reply(&cx)
//! ```
//!
//! Parameters extract in order, inside `dispatch`'s call, so after every guard admits. A
//! `self: Arc<Self>` receiver is passed `::ulo::Dep::into_arc(__ulo_this)`.

use proc_macro2::{Span, TokenStream};
use syn::Ident;

use crate::params::HandlerSig;
use crate::paths::Paths;
use crate::protocol::{HandlerTokens, MetaTokens};

/// One handler's three owed items.
pub struct MountFn<'a> {
    pub tokens: &'a HandlerTokens,
    pub sig: &'a HandlerSig,
    /// The transport's key as a literal, which `__enhancer_specs!` filters scoped entries by. It
    /// equals `<Marker as Transport>::KEY`; the transport crate keeps the two in step.
    pub key: &'a str,
    pub paths: &'a Paths,
    /// An expression evaluating to the transport's handler value, the `H` of `HandlerSpec<T, H>`.
    /// It may name [`call_ident`], bound by the mount function to the call closure.
    pub handler_value: TokenStream,
    /// `.route(..)`'s argument, when the transport has one.
    pub route: Option<TokenStream>,
    /// `.shape(..)`'s argument, when the handler's shape is not `Unary`.
    pub shape: Option<TokenStream>,
}

impl MountFn<'_> {
    /// `__ULO_KEY_<name>`, `__ULO_CHECKS_<name>` and `__ulo_mount_<name>`, as impl items.
    pub fn emit(&self) -> TokenStream {
        todo!("key const, checks const, and the mount fn binding `call_ident()` to `call_closure` before evaluating `handler_value`")
    }
}

/// `__ulo_call`, the binding of the call closure inside the mount function.
pub fn call_ident() -> Ident {
    Ident::new("__ulo_call", Span::mixed_site())
}

/// `__ulo_mount_<name>`.
pub fn mount_fn_ident(name: &str, span: Span) -> Ident {
    Ident::new(&format!("__ulo_mount_{name}"), span)
}

/// `move |cx: <Marker as Transport>::Cx| -> ::ulo::BoxFuture<'static, Result<Reply, ::ulo::BoxError>>
/// { Box::pin(async move { <call_body> }) }`, cloneable into the transport's handler value.
pub fn call_closure(sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let _ = (sig, paths);
    todo!("the closure above around `call_body`")
}

/// The statements of the call: extract each parameter, resolve the controller, call the method,
/// probe the reply. `cx` names the context binding.
pub fn call_body(sig: &HandlerSig, cx: &Ident, paths: &Paths) -> TokenStream {
    let _ = (sig, cx, paths);
    todo!("as in the module documentation")
}

/// `let mut dependencies = ::ulo::Dependencies::default();` followed by each parameter's
/// `<Ty as Param<Marker, _>>::dependencies(&mut dependencies);`, evaluating to `dependencies`.
pub fn dependencies(sig: &HandlerSig, paths: &Paths) -> TokenStream {
    let _ = (sig, paths);
    todo!("a block evaluating to the `Dependencies`")
}

/// A block evaluating to the handler's `::ulo::Metadata`: `controller(expr)` per impl-level
/// declaration and `method(expr)` per method-level one, each spanned at its expression, built
/// once when the handler mounts.
pub fn metadata(meta: &MetaTokens) -> TokenStream {
    let _ = meta;
    todo!("a block evaluating to the `Metadata`")
}
