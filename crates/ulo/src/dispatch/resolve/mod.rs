//! Turning an enhancer declaration into the entries a dispatch target serves with.
//!
//! A declaration names its enhancers three ways (two for an error handler), and all arrive here in
//! the order written.
//! `#[use_guards(MyGuard)]` gives a token that resolves against the role registry, so the enhancer
//! may hold injected dependencies; `#[use_guards(MyGuard{})]` gives a value built where it was
//! written; `#[use_guards(|ctx| ..)]` gives a constructor the framework runs once per execution.
//! A token that resolves against nothing fails `create`, which is what turns a misspelling into a
//! startup refusal rather than a target serving without the guard it declared.
//!
//! One copy, parameterised by the transport. What differs between them is the noun in the
//! diagnostic.

mod grpc;
mod rpc;
mod ws;

pub(crate) use self::grpc::GrpcServiceResolver;
pub(crate) use self::rpc::RpcControllerResolver;
pub(crate) use self::ws::GatewayResolver;

use std::collections::HashMap;

use crate::dispatch::transport::{
    EnhancerRegistry, EnhancerSet, GuardEntry, InterceptorEntry, Transport,
};
use crate::enhancer::{ErrorHandlerDeclaration, GuardDeclaration, InterceptorDeclaration};
use crate::error::SetupResult;

/// What one declaration names, before any of it is resolved: each role in the order written.
pub(crate) struct Declared<T: Transport> {
    pub guards: Vec<GuardDeclaration<T>>,
    pub interceptors: Vec<InterceptorDeclaration<T>>,
    pub error_handlers: Vec<ErrorHandlerDeclaration<T>>,
}

/// Resolve what a dispatch target declares, with the transport's globals ahead of it.
///
/// The globals belong to this tier alone. A handler's own entries stack on what is resolved here,
/// so resolving those with the globals too would run each global twice.
pub(crate) fn resolve_target<T: Transport>(
    registry: &EnhancerRegistry<T>,
    globals: &EnhancerSet<T>,
    declared: Declared<T>,
) -> SetupResult<EnhancerSet<T>> {
    let mut set = globals.clone();
    extend_with(registry, &mut set, declared)?;
    Ok(set)
}

/// What a dispatch target serves with, each key's set already merged.
///
/// `target` is what every key runs: the transport's globals, then the target's own declarations.
/// A key — an RPC pattern, a WebSocket event, a gRPC handler's Rust method name — whose handler
/// declares entries of its own is in `per_key`, holding the target's set with its own entries after
/// it; a key absent there declared nothing and runs `target`. Dispatch borrows the key's set;
/// nothing is merged per call.
pub(crate) struct Resolved<T: Transport> {
    target: EnhancerSet<T>,
    per_key: HashMap<String, EnhancerSet<T>>,
}

impl<T: Transport> Resolved<T> {
    /// The globals, then the target's own declarations. A key with nothing of its own runs this,
    /// and so does a WebSocket connect.
    pub(crate) fn target(&self) -> &EnhancerSet<T> {
        &self.target
    }

    /// The set `key` runs.
    pub(crate) fn for_key(&self, key: &str) -> &EnhancerSet<T> {
        self.per_key.get(key).unwrap_or(&self.target)
    }
}

// Manual, like `Default` below: a derive would bound the marker `T`.
impl<T: Transport> Clone for Resolved<T> {
    fn clone(&self) -> Self {
        Self {
            target: self.target.clone(),
            per_key: self.per_key.clone(),
        }
    }
}

impl<T: Transport> Default for Resolved<T> {
    fn default() -> Self {
        Self {
            target: EnhancerSet::default(),
            per_key: HashMap::new(),
        }
    }
}

/// Resolve a dispatch target's declarations and each of its handlers', with the transport's
/// globals ahead of both.
pub(crate) fn resolve<T: Transport>(
    registry: &EnhancerRegistry<T>,
    globals: &EnhancerSet<T>,
    target: Declared<T>,
    handlers: impl IntoIterator<Item = (String, Declared<T>)>,
) -> SetupResult<Resolved<T>> {
    let target = resolve_target(registry, globals, target)?;
    let mut per_key = HashMap::new();
    for (key, declared) in handlers {
        let mut merged = target.clone();
        merged.extend_from(&resolve_handler(registry, declared)?);
        per_key.insert(key, merged);
    }
    Ok(Resolved { target, per_key })
}

/// Resolve what one handler declares on top of its target's. No globals: they are already in the
/// target-level set this stacks on.
fn resolve_handler<T: Transport>(
    registry: &EnhancerRegistry<T>,
    declared: Declared<T>,
) -> SetupResult<EnhancerSet<T>> {
    let mut set = EnhancerSet::default();
    extend_with(registry, &mut set, declared)?;
    Ok(set)
}

/// Every entry in the order it was written: a token looked up in the registry, a value as the
/// shared entry it already is, a constructor as the per-execution arm.
fn extend_with<T: Transport>(
    registry: &EnhancerRegistry<T>,
    set: &mut EnhancerSet<T>,
    declared: Declared<T>,
) -> SetupResult {
    for guard in declared.guards {
        set.guards.push(match guard {
            GuardDeclaration::Token(token) => registry
                .guards
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>(Role::Guard, &token))?,
            GuardDeclaration::Value(guard) => GuardEntry::Ready(guard),
            GuardDeclaration::Constructor(build) => GuardEntry::Factory(build.0),
        });
    }

    for interceptor in declared.interceptors {
        set.interceptors.push(match interceptor {
            InterceptorDeclaration::Token(token) => registry
                .interceptors
                .get(&token)
                .cloned()
                .ok_or_else(|| not_found::<T>(Role::Interceptor, &token))?,
            InterceptorDeclaration::Value(interceptor) => InterceptorEntry::Ready(interceptor),
            InterceptorDeclaration::Constructor(build) => InterceptorEntry::Factory(build.0),
        });
    }

    for handler in declared.error_handlers {
        set.error_handlers.push(match handler {
            ErrorHandlerDeclaration::Token(token) => {
                registry
                    .error_handlers
                    .get(&token)
                    .cloned()
                    .ok_or_else(|| not_found::<T>(Role::ErrorHandler, &token))?
            }
            ErrorHandlerDeclaration::Value(handler) => handler,
        });
    }

    Ok(())
}

/// The role a token failed to resolve for, with the words the diagnostic needs: the role as the
/// registry names it, the role with its article, and the trait a provider implements to register.
#[derive(Clone, Copy)]
enum Role {
    Guard,
    Interceptor,
    ErrorHandler,
}

impl Role {
    fn name(self) -> &'static str {
        match self {
            Self::Guard => "Guard",
            Self::Interceptor => "Interceptor",
            Self::ErrorHandler => "ErrorHandler",
        }
    }

    fn with_article(self) -> &'static str {
        match self {
            Self::Guard => "A guard",
            Self::Interceptor => "An interceptor",
            Self::ErrorHandler => "An error handler",
        }
    }

    /// The role trait as `T` writes it, such as `Interceptor<HttpContext, Answer<Http>>`.
    fn trait_shape<T: Transport>(self) -> String {
        let context = short_name::<T::Context>();
        let marker = short_name::<T>();
        match self {
            Self::Guard => format!("Guard<{context}>"),
            Self::Interceptor => format!("Interceptor<{context}, Answer<{marker}>>"),
            Self::ErrorHandler => format!("ErrorHandler<{context}, Answer<{marker}>>"),
        }
    }
}

/// A non-generic type's name without its module path.
fn short_name<X: ?Sized>() -> &'static str {
    let full = std::any::type_name::<X>();
    full.rsplit("::").next().unwrap_or(full)
}

/// The one diagnostic a key that resolves against nothing produces.
///
/// A role is registered by the trait impl a declared type carries, or by the trait object a value
/// or factory's slot holds, so a key missing from the registry means the provider is absent from
/// `providers`, does not implement the role, or is a value or factory declared under a slot
/// holding no role. The message names all three.
fn not_found<T: Transport>(role: Role, token: &str) -> Box<dyn std::error::Error + Send + Sync> {
    format!(
        "{transport} {role} '{token}' not found in registry. {subject} registers automatically by \
         implementing {trait_shape}; make sure the provider is in the module's `providers` list. A \
         value or factory registers no role under a slot holding a data type; declare it under a \
         marker holding the role, `key!(Name: dyn {trait_shape})`.",
        transport = T::NAME,
        role = role.name(),
        subject = role.with_article(),
        trait_shape = role.trait_shape::<T>(),
    )
    .into()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::transport::Http;

    /// The article is carried with the role rather than derived from its name: lowercasing the
    /// name gives "A interceptor" and "A errorhandler".
    #[test]
    fn the_diagnostic_names_the_role_with_its_article() {
        let cases = [
            (Role::Guard, "HTTP Guard 'X' not found", "A guard registers"),
            (
                Role::Interceptor,
                "HTTP Interceptor 'X' not found",
                "An interceptor registers",
            ),
            (
                Role::ErrorHandler,
                "HTTP ErrorHandler 'X' not found",
                "An error handler registers",
            ),
        ];
        for (role, opening, subject) in cases {
            let message = not_found::<Http>(role, "X").to_string();
            assert!(message.starts_with(opening), "{message}");
            assert!(message.contains(subject), "{message}");
        }
    }

    /// The trait shape is the one a role marker is written with, so the suggested marker compiles.
    #[test]
    fn the_diagnostic_names_the_role_marker_as_written() {
        let message = not_found::<Http>(Role::Interceptor, "X").to_string();
        assert!(
            message.contains("`key!(Name: dyn Interceptor<HttpContext, Answer<Http>>)`"),
            "{message}"
        );
    }
}
