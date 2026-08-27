//! The one identity a module has: a base plus an optional config fingerprint.
//!
//! The base is the module's type name where the module is a type, or the
//! builder-given string where it is not. The fingerprint folds value
//! configuration in, so two imports of one maker with different config stay
//! distinct while an identical import dedups as a diamond. The rendered key is
//! the registry token, the display string, and the argument
//! `get_module_by_id` accepts — there is no separate name channel.

use std::hash::{Hash, Hasher};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ModuleIdentity {
    base: String,
    fingerprint: Option<u64>,
}

impl ModuleIdentity {
    /// The identity of a module that is a type: its fully-qualified name, the
    /// same canonicalization DI tokens use ([`token_of`](crate::di::token_of)).
    pub fn of_type<M: ?Sized>() -> Self {
        Self {
            base: crate::di::token_of::<M>(),
            fingerprint: None,
        }
    }

    /// The identity of a module that is not a type — a `DynamicModule`'s
    /// builder-given base name.
    ///
    /// `#` followed by sixteen hex digits is how a fingerprint renders, so a
    /// base ending that way would parse as one; any other `#` is fine.
    pub fn named(base: impl Into<String>) -> Self {
        Self {
            base: base.into(),
            fingerprint: None,
        }
    }

    /// Fold value configuration into the identity.
    ///
    /// Order-sensitive: callers with an unordered collection sort it first.
    /// The hash is deterministic across runs of one build, not across
    /// toolchains — a rendered fingerprint is a debugging address, not a value
    /// to keep in source.
    pub fn fingerprinted(mut self, config: &impl Hash) -> Self {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        config.hash(&mut hasher);
        self.fingerprint = Some(hasher.finish());
        self
    }

    /// The registry key: `base`, or `base#<16 hex digits>` when fingerprinted.
    pub fn key(&self) -> String {
        match self.fingerprint {
            Some(fp) => format!("{}#{fp:016x}", self.base),
            None => self.base.clone(),
        }
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    pub fn fingerprint(&self) -> Option<u64> {
        self.fingerprint
    }

    /// Read an identity back out of a rendered key. A trailing `#` plus
    /// sixteen hex digits is a fingerprint; anything else is all base.
    pub fn parse(key: &str) -> Self {
        if let Some((base, suffix)) = key.rsplit_once('#') {
            if suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()) {
                if let Ok(fp) = u64::from_str_radix(suffix, 16) {
                    return Self {
                        base: base.to_string(),
                        fingerprint: Some(fp),
                    };
                }
            }
        }
        Self {
            base: key.to_string(),
            fingerprint: None,
        }
    }
}

impl std::fmt::Display for ModuleIdentity {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.key())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_round_trips_through_parse() {
        let plain = ModuleIdentity::named("SqlxModule");
        assert_eq!(ModuleIdentity::parse(&plain.key()), plain);

        let fingerprinted = ModuleIdentity::named("SqlxModule").fingerprinted(&"postgres://a");
        assert_eq!(ModuleIdentity::parse(&fingerprinted.key()), fingerprinted);
    }

    #[test]
    fn a_hash_that_is_not_a_fingerprint_stays_in_the_base() {
        let id = ModuleIdentity::parse("weird#name");
        assert_eq!(id.base(), "weird#name");
        assert_eq!(id.fingerprint(), None);
    }

    #[test]
    fn config_changes_the_key_and_identical_config_does_not() {
        let a = ModuleIdentity::named("M").fingerprinted(&("x", true));
        let b = ModuleIdentity::named("M").fingerprinted(&("x", true));
        let c = ModuleIdentity::named("M").fingerprinted(&("y", true));
        assert_eq!(a.key(), b.key());
        assert_ne!(a.key(), c.key());
    }
}
