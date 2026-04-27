use proc_macro2::TokenStream;
use syn::{Ident, ItemImpl};

/// Lifecycle hook methods detected via method-level attributes in the impl block.
///
/// Each field holds the name of the user's method annotated with the corresponding
/// attribute (`#[on_module_init]`, `#[on_module_destroy]`, etc.), or `None` if not used.
#[derive(Debug, Clone, Default)]
pub struct LifecycleHooks {
    pub on_module_init: Option<Ident>,
    pub on_application_bootstrap: Option<Ident>,
    pub on_module_destroy: Option<Ident>,
    pub before_application_shutdown: Option<Ident>,
    pub on_application_shutdown: Option<Ident>,
}

impl LifecycleHooks {
    pub fn has_any(&self) -> bool {
        self.first_annotated().is_some()
    }

    /// Return the first annotated hook in declaration order, used to anchor compile errors
    /// at the user's attribute span when the surrounding scope forbids lifecycle hooks.
    pub fn first_annotated(&self) -> Option<&Ident> {
        self.on_module_init
            .as_ref()
            .or(self.on_application_bootstrap.as_ref())
            .or(self.on_module_destroy.as_ref())
            .or(self.before_application_shutdown.as_ref())
            .or(self.on_application_shutdown.as_ref())
    }
}

/// Emit a `compile_error!` at the first annotated hook's span if any hook is present.
///
/// Used by request/transient provider generators where lifecycle hooks have no
/// well-defined firing point. The message argument explains why the surrounding
/// scope rejects hooks.
pub fn reject_lifecycle_hooks(hooks: &LifecycleHooks, message: &str) -> Option<TokenStream> {
    hooks
        .first_annotated()
        .map(|ident| syn::Error::new(ident.span(), message).to_compile_error())
}

/// Scan the impl block for methods annotated with lifecycle hook attributes.
///
/// Signal-bearing hooks (`before_application_shutdown`, `on_application_shutdown`)
/// must have the signature `async fn name(&self, signal: Option<String>)`.
pub fn detect_lifecycle_hooks(impl_block: &ItemImpl) -> LifecycleHooks {
    let mut hooks = LifecycleHooks::default();

    for item in &impl_block.items {
        if let syn::ImplItem::Fn(method) = item {
            for attr in &method.attrs {
                if let Some(ident) = attr.path().get_ident() {
                    let method_name = method.sig.ident.clone();
                    match ident.to_string().as_str() {
                        "on_module_init" => hooks.on_module_init = Some(method_name),
                        "on_application_bootstrap" => {
                            hooks.on_application_bootstrap = Some(method_name)
                        }
                        "on_module_destroy" => hooks.on_module_destroy = Some(method_name),
                        "before_application_shutdown" => {
                            hooks.before_application_shutdown = Some(method_name)
                        }
                        "on_application_shutdown" => {
                            hooks.on_application_shutdown = Some(method_name)
                        }
                        _ => {}
                    }
                }
            }
        }
    }

    hooks
}

/// Strip lifecycle hook attributes from methods in an impl block before emitting.
///
/// These attributes are consumed by the macro; they must not appear in the final output
/// or the compiler will reject them as unknown attributes.
pub fn strip_lifecycle_attrs(impl_block: &ItemImpl) -> ItemImpl {
    const LIFECYCLE_ATTRS: &[&str] = &[
        "on_module_init",
        "on_application_bootstrap",
        "on_module_destroy",
        "before_application_shutdown",
        "on_application_shutdown",
    ];

    let mut cleaned = impl_block.clone();
    for item in &mut cleaned.items {
        if let syn::ImplItem::Fn(method) = item {
            method.attrs.retain(|attr| {
                !attr
                    .path()
                    .get_ident()
                    .map(|id| LIFECYCLE_ATTRS.contains(&id.to_string().as_str()))
                    .unwrap_or(false)
            });
        }
    }
    cleaned
}
