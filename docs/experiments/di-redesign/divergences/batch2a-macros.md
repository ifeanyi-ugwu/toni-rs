# Divergences: race 2a's pre-compile batch, the macros (T5)

T5 as signed off (`transports/DIVERGENCES.md` T5, `race2a-MX.md` entry 7): `#[routes]` copies each
handler's `cfg` attributes onto every item that names the handler's generated items, the key
assertion becomes a block of `cfg`-gated statements, and `keys::assertions` takes the handlers'
`cfg` attributes. Each entry below gives what the sign-off says, what was written, and why.

Files changed: `crates/ulo-handler-codegen/src/{cfg.rs (new), keys.rs, lib.rs}`,
`crates/ulo-macros/src/routes/mod.rs`, `crates/ulo-macros/src/lib.rs` (the `#[routes]` doc).

## The signature

```rust
// ulo_handler_codegen::keys
pub struct GatedHandler<'a> {
    pub name: &'a Ident,
    pub gates: &'a [Attribute],
}

pub fn assertions(self_ty: &Type, generics: &Generics, keys: &[Ident], handlers: &[GatedHandler<'_>]) -> Assertions;

// ulo_handler_codegen::cfg (new)
pub fn presence_gates(attrs: &[Attribute]) -> Vec<Attribute>;
```

`#[routes]` is the one caller. `ulo-http-macros` and `ulo-transport-macros` call neither function;
`ulo-http-macros` uses `emit`, `params`, `protocol` and `reply`, none of which changed.

## Entries

### 1. Which attributes count as a handler's gates

- **Sign-off:** the handler's `cfg` attributes, and `cfg_attr` "where it gates the handler's
  presence".
- **Written:** `cfg::presence_gates` returns every `cfg` as written, and every `cfg_attr` whose
  expansion holds a `cfg` at any depth, reduced to the branches that hold one:
  `#[cfg_attr(test, cfg(unix), inline)]` is copied as `#[cfg_attr(test, cfg(unix))]`. A
  `cfg_attr` holding no `cfg`, such as `#[cfg_attr(feature = "x", inline)]`, is no gate. An inner
  `#![cfg(..)]` in the method's body is a gate too, and its copy is written as an outer attribute.
  Only a single-segment `cfg` or `cfg_attr` path counts, matching what rustc treats as one.
- **Why:** copying `inline` onto a `const _` fails to compile where the predicate holds (E0518),
  and the reduction keeps only what decides presence. syn places a method body's inner attributes
  in `ImplItemFn::attrs`; rustc removes a method whose body opens with `#![cfg(any())]` (probed on
  1.88 against a duplicate-definition control). The reduction splits `cfg_attr`'s arguments at
  top-level commas and reads only each piece's leading name, so no attribute inside is parsed and
  one that is no `cfg` is dropped whatever its form.

### 2. What each emitted item looks like

For a handler `get_rpc` behind `#[cfg(feature = "rpc")]`, beside an ungated `index`, in a
non-generic controller with `#[guards(http = AuthGuard)]`:

```rust
impl UsersController {
    // the methods as written, each with `__handler` appended; `get_rpc` keeps its `#[cfg]`
}

impl ::ulo::Controller for UsersController {
    fn mount(m: &mut ::ulo::Mount<'_>) {
        let __ulo_shared = ::ulo::__private::Shared(());
        Self::__ulo_mount_index(m, &__ulo_shared);
        #[cfg(feature = "rpc")]
        Self::__ulo_mount_get_rpc(m, &__ulo_shared);
    }
}

const _: () = {
    let __ulo_found = ::ulo::__private::key_in("http", &[<UsersController>::__ULO_KEY_index]);
    #[cfg(feature = "rpc")]
    let __ulo_found = __ulo_found || ::ulo::__private::key_in("http", &[<UsersController>::__ULO_KEY_get_rpc]);
    ::core::assert!(__ulo_found, "`http` is not the key of any handler's transport in this impl (handlers: index; behind `#[cfg]`: get_rpc)");
};
const _: () = <UsersController>::__ULO_CHECKS_index;
#[cfg(feature = "rpc")]
const _: () = <UsersController>::__ULO_CHECKS_get_rpc;
```

The generic form writes the same block over `Self` as the associated
`const __ULO_KEYS_CHECK_http: () = { .. };`, read unconditionally from `mount`, and each
`let () = Self::__ULO_CHECKS_<name>;` in `mount` under its handler's gates.

- **Written:** the ungated handlers' keys stay in one `key_in` over an array in the first `let`;
  each gated handler adds one `let` under its gates. `__ulo_found` is mixed-site.
- **Why:** a shadowing `let` needs no `mut`, so a build in which every gated statement is compiled
  out raises no `unused_mut`.

### 3. A scoped key whose handlers are all compiled out fails the assertion in that build

- **Sign-off:** "a cfg'd-out handler's key is not listed".
- **Written:** read strictly. With `#[guards(rpc = RpcAuth)]` on the impl and every `rpc` handler
  behind `#[cfg(feature = "rpc")]`, a build without the feature fails with "`rpc` is not the key
  of any handler's transport in this impl (handlers: index; behind `#[cfg]`: get_rpc)", spanned on
  `rpc`. The entry belongs behind the same `cfg`: `#[cfg_attr(feature = "rpc", guards(rpc = ..))]`
  on the impl, which rustc evaluates before `#[routes]` runs even when written below it. The
  `#[routes]` doc says so.
- **Why:** the refusal table refuses "a controller-level transport key that matches no handler",
  and in that build none matches. The alternative, counting a compiled-out handler as a possible
  match, emits each gate's negation and turns off the misspelling check in every build that
  compiles out any gated handler of the impl. The remedy was probed on rustc 1.88: a `cfg_attr`
  on the item below an attribute macro reaches the macro already evaluated, while a method's
  `cfg` and `cfg_attr` reach it unevaluated.

### 4. The assertion's message lists gated handlers apart

- **Written:** "(handlers: index; behind `#[cfg]`: get_rpc)"; "(handlers behind `#[cfg]`: get_rpc)"
  when every handler is gated; the two earlier forms, "(handlers: ..)" and "(no handlers)", are
  unchanged when no handler is gated.
- **Why:** a build that fails the assertion is often one that compiled the listed gated handler
  out, and the message is a literal fixed before `cfg` is evaluated. Listing `get_rpc` among the
  handlers would name a handler that build does not have.

### 5. `Controller::mount`'s parameter is `_m` when every mount call is gated

- **Written:** the parameter is `m` when some handler is ungated, `_m` otherwise (no handlers
  included, as before).
- **Why:** a build compiling out every gated handler would otherwise warn on an unused `m`.
  `__ulo_shared`, still built in that build, carries a leading underscore.

## Not covered

- A transport attribute written inside `cfg_attr`, `#[cfg_attr(feature = "x", ulo_http::get(..))]`.
  `cfg_attr` is in `attrs::is_inert`, so `#[routes]` does not see the method as a handler and
  appends no `__handler`; with the feature on, the HTTP attribute fails with "#[get] goes on a
  method of a `#[routes]` impl". Supporting it means reading through `cfg_attr` for the transport
  attribute and appending `__handler` inside a matching `cfg_attr`. The failure is a compile
  error, not a wrong mount.
- An enhancer or `#[meta]` attribute inside `cfg_attr` on a method. `#[routes]` does not take it,
  and where the predicate holds it expands to `#[guards(..)]` after `#[routes]` has run, which
  the marker macro refuses. A compile error, as above. On the impl it works (entry 3).

## Verification

No cargo run, per the batch rules. With standalone `rustc`, output to the session scratchpad and
nothing written to the repository or `target/`:

- `ulo-handler-codegen` (rlib) and `ulo-macros` (proc-macro) compile with no warning on rustc
  1.98.1 against the `syn` 2.0.118, `quote` 1.0.46 and `proc-macro2` 1.0.106 rlibs already in
  `target/debug/deps`, the toolchain that built them.
- The built `ulo_macros` ran end to end against a stub `ulo` crate and a stub transport attribute
  writing the three owed items, with `feature = "rpc"` off and on, under `#![deny(warnings)]`
  (`dead_code` and `unused_imports` allowed for the stubs): a non-generic controller mixing
  ungated, `cfg`-gated, reduced-`cfg_attr` and `cfg_attr(.., inline)` handlers; an impl whose every
  handler is gated, one named `r#type`; a generic controller instantiated in `mount`. Each
  compiles in both builds.
- Against known violations: the all-gated scoped key fails off and passes on, for both the free
  and the generic form; a misspelled `htpp` fails in both builds; the `cfg_attr` remedy passes in
  both.
- The hand-expanded shapes, `cfg` and `cfg_attr` on a `let` in a `const` block, on an expression
  statement, on a free `const _`, and on a `let` in `mount`, compile on rustc 1.88.

## Requests

None. `ulo-http-macros` does not call `keys::assertions`.
