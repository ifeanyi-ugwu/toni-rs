//! The scope word, written the same way wherever a scope is declared: `#[injectable(execution)]`
//! on a type, `with(execution) = ..` on a closure. Defined beside the `__handler` grammar in
//! `ulo-handler-codegen`, which an enhancer entry's `with(<scope>)` belongs to.

pub(crate) use ulo_handler_codegen::protocol::ScopeArg;
