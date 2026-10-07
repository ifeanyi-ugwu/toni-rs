//! `#[gateway(..)]`: the impl's `GatewayConfig`, its `mount_gateway` probing the connection hooks
//! at the concrete type.
//!
//! The impl passes through unchanged and is followed by
//!
//! ```text
//! impl ::ulo_ws::GatewayConfig for ChatGateway {
//!     fn settings() -> ::ulo_ws::GatewaySettings { ::ulo_ws::GatewaySettings::at("/chat").namespace("lobby") }
//!     fn connect_guards(spec: &mut ::ulo::EnhancerSpec<::ulo_ws::WsConnect>) { spec.guard::<TokenGuard>(); }
//!     fn session() -> ::ulo_ws::SessionFactory { ::ulo_ws::SessionFactory::default_of::<ChatSession>() }
//!     fn mount_gateway(m: &mut ::ulo::Mount<'_>) {
//!         let hooks = ::ulo_ws::__private::Hooks::<Self>::new()
//!             .on_connect((&&::ulo_ws::__private::HookProbe::<Self>::new()).on_connect())
//!             /* on_disconnect, after_init likewise */;
//!         ::ulo_ws::__private::mount_connect::<Self>(m, hooks);
//!     }
//! }
//! ```
//!
//! with the probe traits imported anonymously.

use proc_macro2::{Span, TokenStream};
use quote::{ToTokens, quote, quote_spanned};
use syn::parse::{Parse, ParseStream};
use syn::punctuated::Punctuated;
use syn::spanned::Spanned;
use syn::{Attribute, Expr, ExprClosure, Ident, ImplItem, ItemImpl, Lit, LitStr, Meta, Token, Type, parenthesized};
use ulo_handler_codegen::protocol::{Entry, Form};
use ulo_handler_codegen::util::{check_factory_params, combine, wrap_async};

pub(crate) fn expand(attr: TokenStream, item: TokenStream) -> syn::Result<TokenStream> {
    let args: Args = syn::parse2(attr)?;
    let item: ItemImpl = syn::parse2(item)?;
    if item.trait_.is_some() {
        return Err(syn::Error::new_spanned(
            &item.self_ty,
            "#[ulo_ws::gateway] goes on the `#[routes]` impl holding the `#[ulo_ws::message]` handlers, not on a trait impl",
        ));
    }
    if !item.items.iter().any(|entry| matches!(entry, ImplItem::Fn(method) if method.attrs.iter().any(is_message))) {
        return Err(syn::Error::new(
            Span::call_site(),
            "a gateway with no `#[ulo_ws::message]` handler mounts nothing; add one, or implement `ulo_ws::Gateway` by hand \
             for a gateway that reads every message raw",
        ));
    }
    let config = args.config()?;
    let (impl_generics, _, where_clause) = item.generics.split_for_impl();
    let self_ty = &item.self_ty;
    Ok(quote! {
        #item

        impl #impl_generics ::ulo_ws::GatewayConfig for #self_ty #where_clause {
            #config

            fn mount_gateway(__ulo_m: &mut ::ulo::Mount<'_>) {
                #[allow(unused_imports)]
                use ::ulo_ws::__private::{
                    NoAfterInit as _, NoOnConnect as _, NoOnDisconnect as _, ViaAfterInit as _, ViaOnConnect as _,
                    ViaOnDisconnect as _,
                };
                let __ulo_hooks = ::ulo_ws::__private::Hooks::<Self>::new()
                    .on_connect((&&::ulo_ws::__private::HookProbe::<Self>::new()).on_connect())
                    .on_disconnect((&&::ulo_ws::__private::HookProbe::<Self>::new()).on_disconnect())
                    .after_init((&&::ulo_ws::__private::HookProbe::<Self>::new()).after_init());
                ::ulo_ws::__private::mount_connect::<Self>(__ulo_m, __ulo_hooks);
            }
        }
    })
}

/// Whether `attr` is a message attribute, written plainly or inside a `cfg_attr`. Read by the
/// attribute's last path segment: `#[routes]` has not expanded the methods' attributes when this
/// runs, so the transport attribute is still present as written.
fn is_message(attr: &Attribute) -> bool {
    let named = |path: &syn::Path| path.segments.last().is_some_and(|segment| segment.ident == "message");
    if named(attr.path()) {
        return true;
    }
    if !attr.path().is_ident("cfg_attr") {
        return false;
    }
    attr.parse_args_with(Punctuated::<Meta, Token![,]>::parse_terminated)
        .map(|metas| metas.iter().skip(1).any(|meta| named(meta.path())))
        .unwrap_or(false)
}

/// One word setting's accepted words and the expression each writes.
fn word(key: &Ident, value: &Ident, choices: &[(&str, TokenStream)]) -> syn::Result<TokenStream> {
    let written = value.to_string();
    choices.iter().find(|(word, _)| *word == written).map(|(_, tokens)| tokens.clone()).ok_or_else(|| {
        let words: Vec<String> = choices.iter().map(|(word, _)| format!("`{word}`")).collect();
        syn::Error::new(value.span(), format!("`{key}` takes {}", words.join(" or ")))
    })
}

#[derive(Default)]
struct Args {
    path: Option<LitStr>,
    setters: Vec<TokenStream>,
    guards: Option<(Ident, Vec<Entry>)>,
    session: Option<TokenStream>,
    seen: Vec<String>,
    errors: Vec<syn::Error>,
}

impl Parse for Args {
    fn parse(input: ParseStream<'_>) -> syn::Result<Self> {
        let mut args = Args::default();
        while !input.is_empty() {
            let key: Ident = input.parse()?;
            let name = key.to_string();
            if args.seen.contains(&name) {
                args.errors.push(syn::Error::new(key.span(), format!("`{name}` is written twice")));
            }
            args.seen.push(name.clone());
            if name == "connect_guards" {
                let content;
                parenthesized!(content in input);
                let entries = Punctuated::<Entry, Token![,]>::parse_terminated(&content)?;
                args.guards = Some((key, entries.into_iter().collect()));
            } else {
                input.parse::<Token![=]>()?;
                args.setting(&key, input)?;
            }
            if input.is_empty() {
                break;
            }
            input.parse::<Token![,]>()?;
        }
        Ok(args)
    }
}

impl Args {
    fn setting(&mut self, key: &Ident, input: ParseStream<'_>) -> syn::Result<()> {
        let name = key.to_string();
        let span = key.span();
        match name.as_str() {
            "path" => {
                let path: LitStr = input.parse()?;
                if !path.value().starts_with('/') {
                    self.errors.push(syn::Error::new(path.span(), "a gateway path starts with `/`"));
                }
                self.path = Some(path);
            }
            "namespace" | "event" => {
                let value: LitStr = input.parse()?;
                let setter = Ident::new(&name, span);
                self.setters.push(quote!(.#setter(#value)));
            }
            "codec" => {
                let value: Ident = input.parse()?;
                let codec = word(key, &value, &[
                    ("json", quote!(::ulo_ws::Codec::Json)),
                    ("msgpack", quote!(::ulo_ws::Codec::MsgPack)),
                ])?;
                self.setters.push(quote!(.codec(#codec)));
            }
            "refuse" => {
                let value: Ident = input.parse()?;
                let refuse = word(key, &value, &[
                    ("close", quote!(::ulo_ws::Refuse::Close)),
                    ("handshake", quote!(::ulo_ws::Refuse::Handshake)),
                ])?;
                self.setters.push(quote!(.refuse(#refuse)));
            }
            "overflow" => {
                let value: Ident = input.parse()?;
                let overflow = word(key, &value, &[
                    ("close", quote!(::ulo_ws::Overflow::Close)),
                    ("drop_oldest", quote!(::ulo_ws::Overflow::DropOldest)),
                ])?;
                self.setters.push(quote!(.overflow(#overflow)));
            }
            "port" => {
                let value: Ident = input.parse()?;
                let port = word(key, &value, &[("http", quote!(::ulo_ws::Port::Http)), ("own", quote!(::ulo_ws::Port::Own))])?;
                self.setters.push(quote!(.port(#port)));
            }
            "subprotocols" => {
                let content;
                syn::bracketed!(content in input);
                let names = Punctuated::<LitStr, Token![,]>::parse_terminated(&content)?;
                if !names.is_empty() {
                    let names = names.iter();
                    self.setters.push(quote!(.subprotocols([#(#names),*])));
                }
            }
            "message_limit" => {
                let value: Expr = input.parse()?;
                self.setters.push(quote_spanned!(value.span()=> .message_limit(#value)));
            }
            "max_connections" | "max_inflight" | "max_outbound" => {
                let value: Expr = input.parse()?;
                let setter = Ident::new(&name, span);
                let count = match &value {
                    Expr::Lit(literal) if matches!(literal.lit, Lit::Int(_)) => {
                        quote_spanned!(value.span()=> ::ulo_ws::__private::transport::Count::Max(#value))
                    }
                    _ => value.to_token_stream(),
                };
                self.setters.push(quote!(.#setter(#count)));
            }
            "ping_interval" | "pong_timeout" => {
                let value: Expr = input.parse()?;
                let setter = Ident::new(&name, span);
                self.setters.push(quote_spanned!(value.span()=> .#setter(#value)));
            }
            "session" => {
                let ty: Type = input.parse()?;
                self.session(key, quote_spanned!(ty.span()=> ::ulo_ws::SessionFactory::default_of::<#ty>()));
            }
            "session_with" => {
                let closure: ExprClosure = input.parse()?;
                check_factory_params(&closure)?;
                let closure = wrap_async(&closure);
                self.session(key, quote!(::ulo_ws::SessionFactory::with(#closure)));
            }
            _ => {
                return Err(syn::Error::new(
                    span,
                    "expected `path`, `namespace`, `event`, `codec`, `subprotocols`, `session`, `session_with`, \
                     `connect_guards(..)`, `refuse`, `overflow`, `port`, `message_limit`, `max_connections`, `max_inflight`, \
                     `max_outbound`, `ping_interval` or `pong_timeout`",
                ));
            }
        }
        Ok(())
    }

    fn session(&mut self, key: &Ident, factory: TokenStream) {
        if self.session.is_some() {
            self.errors.push(syn::Error::new(key.span(), "`session` and `session_with` are two spellings of one setting; write one"));
        }
        self.session = Some(factory);
    }

    /// `settings`, and `connect_guards` and `session` where written.
    fn config(mut self) -> syn::Result<TokenStream> {
        let Some(path) = self.path.take() else {
            self.errors.push(syn::Error::new(Span::call_site(), "a gateway takes its path, as in `#[ulo_ws::gateway(path = \"/chat\")]`"));
            return Err(combine(std::mem::take(&mut self.errors)).unwrap_or_else(|| syn::Error::new(Span::call_site(), "")));
        };
        let guards = match self.guards.take() {
            None => TokenStream::new(),
            Some((key, entries)) => {
                let spec = Ident::new("__ulo_spec", key.span());
                let mut calls = Vec::new();
                for entry in &entries {
                    match guard_call(entry) {
                        Ok(call) => calls.push(quote!(#spec #call;)),
                        Err(error) => self.errors.push(error),
                    }
                }
                quote! {
                    fn connect_guards(#spec: &mut ::ulo::EnhancerSpec<::ulo_ws::WsConnect>) {
                        #(#calls)*
                    }
                }
            }
        };
        if let Some(error) = combine(std::mem::take(&mut self.errors)) {
            return Err(error);
        }
        let setters = &self.setters;
        let session = self.session.as_ref().map(|factory| {
            quote! {
                fn session() -> ::ulo_ws::SessionFactory {
                    #factory
                }
            }
        });
        Ok(quote! {
            fn settings() -> ::ulo_ws::GatewaySettings {
                ::ulo_ws::GatewaySettings::at(#path) #(#setters)*
            }

            #guards

            #session
        })
    }
}

/// The `EnhancerSpec<WsConnect>` call for one connect guard: by type, by value, or by closure in
/// its scope, a closure's body wrapped in `async move` as `#[guards]` wraps one. A transport key
/// names nothing here: every entry is a connect guard.
fn guard_call(entry: &Entry) -> syn::Result<TokenStream> {
    if let Some(transport) = &entry.transport {
        return Err(syn::Error::new(
            transport.span(),
            "every `connect_guards(..)` entry is a `Guard<WsConnect>`; it takes no transport key",
        ));
    }
    Ok(match &entry.form {
        Form::Type(ty) => quote_spanned!(ty.span()=> .guard::<#ty>()),
        Form::Value(expr) => quote_spanned!(expr.span()=> .guard_value(#expr)),
        Form::With { scope: None, closure } => {
            let built = wrap_async(closure);
            quote_spanned!(closure.span()=> .guard_with(#built))
        }
        Form::With { scope: Some(scope), closure } => {
            let scope = scope.path();
            let built = wrap_async(closure);
            quote_spanned!(closure.span()=> .guard_with_in::<#scope, _, _>(#built))
        }
    })
}
