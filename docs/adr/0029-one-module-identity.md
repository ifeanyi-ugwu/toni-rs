# 0029 — A module has one identity, and display derives from it

Status: accepted

## Context

`ModuleMetadata` carried two identity methods. `get_id` was the registry key — a type name for
static modules, `base#fingerprint` for dynamic ones (and, since ADR-0028's follow-ups, for the
GraphQL modules). `get_name` was a display string, hand-written per module in formats that never
agreed: the bare ident for `#[module]` types, `ConfigModule<full::path>` for toni-config,
the constant `"GraphQLModule"` for both GraphQL crates, the builder-given base for `DynamicModule`.

The name channel carried no information the id lacked. A `DynamicModule` is given its name at
`builder("SqlxModule")` — the same string that opens its id. For every typed module the name was a
re-derivation of the type the id already spells. And where the name was supposed to earn its keep
it failed: the global-export clash message printed the display names of the two colliding modules,
which for two configs of one maker are identical — the fingerprinted ids it did not print were the
only strings that distinguished them. The by-name lookup added with the module-handle API inherited
the same defect: two same-type GraphQL modules share the name `"GraphQLModule"`, so neither type
nor name could reach one of them.

This is the shape ADR-0028 removed from the token namespace: one fact, several hand-maintained
renderings, no owner.

## Decision

A module has one identity: `ModuleIdentity { base, fingerprint: Option<u64> }`, returned by the
single trait method `identity()`. The base is `token_of::<Self>()` where the module is a type and
the builder-given string where it is not; the fingerprint folds value configuration in, hashed by
`ModuleIdentity::fingerprinted` — the one hasher, replacing the copies in `DynamicModule` and both
GraphQL crates. `get_id` and `get_name` are gone.

The rendered key (`base` or `base#<16 hex digits>`) is the registry token, the display string
(`Display` prints it; the scan log and the clash refusal show it), and an address:
`get_module_by_id` accepts a full key exactly or a bare base with ambiguity detection, replacing
`get_module_by_name`. An ambiguity error lists full keys, so the case that was unreachable — one
of two same-type, same-name modules — is reached by pasting the key the error printed. Identity
derives from configuration, so a module value built with the same config renders the same key.

Display is the key. Full type paths in logs are verbose; verbose-and-unambiguous
beats short-and-collapsing, and a derived `short()` is one function to add if log noise becomes
real — not a stored per-module string that can drift again.

## Consequences

- One method to implement, and misimplementing the format is no longer possible: modules supply a
  base and config parts; the framework renders.
- The clash refusal and the scan log distinguish two configs of one maker, which the name-based
  display never did.
- Hand-written `ModuleMetadata` impls break: replace two methods with `identity()`.
- The fingerprint is deterministic across runs of one build, not across toolchains. A rendered key
  is a debugging address; source code addresses modules by type or by base.
- `Module` no longer stores a name; the identity key is the only module string the container holds.
