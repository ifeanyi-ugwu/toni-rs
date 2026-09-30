# DI redesign (experiment)

An end-to-end redesign of ulo's dependency injection, written from a list of required capabilities
without reference to the current implementation. Nothing here is scheduled or built; this branch
holds it until it is either phased into ulo or dropped.

- `CAPABILITIES.md` — the 48 capabilities and the constraints the design answers. The bracketed
  numbers in the design, such as [12], refer to this list.
- `DESIGN.md` — the design. `fw` stands for the framework's crate prefix, `ulo`. Section 14 ends
  at item 7.
