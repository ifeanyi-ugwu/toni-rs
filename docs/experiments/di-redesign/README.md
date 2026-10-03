# DI redesign (experiment)

An end-to-end redesign of ulo's dependency injection, written from a list of required capabilities
without reference to the previous implementation, and a rebuild of the core to it.

On this branch `crates/ulo` and `crates/ulo-macros` are rewritten to `DESIGN.md`, and the root
workspace is narrowed to those two crates; every other crate is excluded until it is ported. The
rebuild has not been compiled yet.

- `CAPABILITIES.md` — the 48 capabilities and the constraints the design answers. The bracketed
  numbers in the design, such as [12], refer to this list.
- `DESIGN.md` — the design. `fw` stands for the framework's crate prefix, `ulo`.
- `REFINEMENTS.md` — the first review of the design, each claim marked by how it was established.
- `SHUTDOWN_REVIEW.md` — the review of the shutdown phases, per-hook timeouts and terminal
  executions.
- `RESPONSE.md` — the design author's responses to the reviews and to the rebuild's divergences, in
  order, each with the user's sign-off.
- `BUILD_PLAN.md` — the rebuild's areas, the files each owns, and the contracts between them.
- `DIVERGENCES.md` — every place the rebuild departs from the design or fills a gap it leaves,
  grouped by wave, with the user's decisions on each.
- `divergences/` — the per-area logs `DIVERGENCES.md` is assembled from.
- `probes/` — the scratch crate behind the reviews, outside the ulo workspace; each binary shows
  one claim, and a `*_fails.rs` binary's compile error is its result.
