Design a dependency-injection system for a Rust application framework. Below is what it must be able to do and the constraints it lives under. Nothing here describes an existing implementation; design it from these requirements.

**The framework it serves.** A NestJS-shaped Rust framework. Applications are built from modules holding providers (services) and controllers. Controllers are dispatch targets on four transports: HTTP, WebSocket, RPC (over TCP, UDP and message brokers such as NATS, Redis, RabbitMQ, MQTT and Kafka) and gRPC. Around every handler run enhancers: guards, interceptors and error handlers, plus HTTP middleware. The same container also runs with no transport, for a job, a CLI command or a test. Integration crates (databases, caches, WebSocket broadcast, configuration, health checks, GraphQL) plug in as modules.

**Constraints**
- Stable Rust 1.88, edition 2024; nothing nightly.
- Async throughout. The application value is `Send`, so it can move across threads.
- The core crate depends on no async runtime; transports and integrations bring their own.
- Every macro is sugar over a public value-level API that integration crates call directly, without macros.
- Shared instances cross threads, so they are `Send + Sync`.

**Stated preferences**
- A dependency at a site is spelled `Dep<T>`.
- Modules are distinct Rust types.

**Capabilities**

Bindings
1. Declare a type as a service the container builds, with its dependencies injected.
2. Bind a value (a constant, a config struct) under a key.
3. Bind an async factory. Its parameters are its dependencies, and its output is the binding.
4. Bind an implementation under a trait, and inject it by the trait.
5. Hold several bindings of one type and tell them apart (a primary and a replica database).
6. Make one binding reachable under a second key (an alias).
7. Collect many contributions under one trait and inject them as a list (plugins, a set of global guards). Contributions can come from several modules.
8. Bind a third-party type the application can neither annotate nor implement traits for (a driver's connection pool).
9. Keys are checked by the compiler; there are no free-form string keys.
10. A second single binding under one key is refused rather than silently replacing the first. Mixing a single binding and a collection under one key is refused too.

Injection sites
11. Fields, constructor parameters and factory parameters follow one rule.
12. A site can read a service, a trait object, a collection, or a typed view of per-execution data (for example, the current user a guard stored for the handler).
13. Optional dependencies.
14. Owned state beside dependencies, set by the constructor.
15. Fallible and async constructors, whose failure is reported as a startup error.
16. A site is read by its type, not its spelling: an alias or a renamed import reads the same binding.
17. Every holder of a shared instance holds the same object, so one holder's mutation, or a startup hook's effect, is visible to all. Services need not be `Clone`.

Scopes and executions
18. Singleton: built once at startup and shared across the application.
19. Execution scope: built once per execution and shared inside it. An execution is one HTTP request, one WebSocket message, one RPC call, one gRPC call, or one standalone execution opened by code. A WebSocket connection is not an execution; per-connection state lives in a session.
20. Transient: a fresh instance at every site.
21. A singleton that depends on an execution-scoped binding is refused at startup.
22. A controller that depends on an execution-scoped binding is built per call, automatically.
23. An execution carries a per-execution cache, a typed extension bag (written by guards and middleware, read by handlers), a cancellation signal and an optional deadline. The current request or call context can be injected into execution-scoped services.

Enhancers and roles
24. Guards, interceptors, error handlers, middleware and WebSocket gateways take part in the container, as singletons or built per execution.
25. A type's role comes from the traits it implements, per transport; an HTTP guard and an RPC guard are different roles. There are no marker attributes.
26. An enhancer is declared by type (resolved from the container), by value (built once and shared), or by closure. A closure's scope follows the same rule as a type's: built once unless something it reads needs an execution, and an explicit scope overrides that, so a closure that creates per-call state declares itself per execution. Enhancers stack in the order global, then per controller, then per method.
27. Within one call, each guard is built only after the one before it admits, and nothing below the guards is built until all of them admit.
28. A binding under a key whose declared type is a role (for example, "a guard for HTTP") registers that role.

Modules
29. A module declares imports, providers, controllers and exports. An export is the only way a binding leaves its module.
30. Global modules, whose exports reach every module.
31. Modules configured by value (`DbModule::for_root(url)`). One module type with two configurations is two modules; the same configuration imported twice (a diamond) is one.
32. Several keyed instances of one configurable module (two databases from one integration), each injected by its key.
33. Modules built at runtime by a function, for integration crates.
34. A module can re-export what one of its imports exports.
35. Module identity is used for deduplication and is printed as a readable name in every error.
36. Middleware configured per module, by route.
37. A handle to the current module, or to a named one, for runtime lookups limited to what that module can see.
38. Loading a module lazily, after startup.

Lifecycle
39. Async hooks. At startup: module init, then application bootstrap; either can fail, failing startup. At close: module destroy, before-shutdown and shutdown, the last two receiving the signal name. A module can have hooks of its own.
40. Startup hooks run in dependency order, so a provider's hook runs after the hooks of what it depends on. Shutdown runs in reverse.
41. Hooks are for singletons only; a hook on an execution-scoped or transient type is refused at compile time.
42. Startup reachability checks for external resources (a database ping), with retries and a timeout. A failed check fails startup, with credentials redacted from the error.
43. Startup phases stay separate: build the graph and connect outbound dependencies, then bind sockets, then serve.

Errors and diagnostics
44. Every wiring error is reported before any instance is built or any connection opened, all of them in one pass: a missing dependency, two sources for one key (naming both modules), a scope violation, a duplicate binding, a dependency cycle (printing the full path).
45. Wiring errors are caught at compile time where feasible, and at startup otherwise. Nothing panics or exits the process; startup returns a typed error.
46. A compile error points at the offending site and says what to write instead.
47. Runtime lookups work from the application, a standalone context or a module handle, with or without an execution. They return a typed error a caller can branch on: not found, wrong type, execution required, ambiguous module.

Testing
48. A test can build a module graph with some bindings overridden (a mock repository) without editing the production modules.

**What to deliver.**
- The user-facing API, with a short example per capability group.
- The core types and traits.
- How keys, bindings, sites, scopes, executions and modules fit together.
- What is refused, and where: compile time, startup or runtime.
- The surface an integration crate writes against without macros.
