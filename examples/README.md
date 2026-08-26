# toni examples

Each file here runs on its own with `cargo run --example <name>`. Start with
[hello_world.rs](hello_world.rs) for the smallest working app, or with the guide below if you are
arriving from NestJS.

## Parsing, validating and refusing input

[validation_complete_guide.rs](validation_complete_guide.rs) is the one to read if you are looking
for pipes. toni has no pipe: what `PipeTransform` does — receive the value a handler is about to be
given, then reshape or refuse it — is what an extractor does here, so the rules that apply are
visible in the handler's signature.

```bash
cargo run --example validation_complete_guide
```

It serves HTTP on `127.0.0.1:3000`, RPC on `127.0.0.1:3001` and a WebSocket gateway at `/ws`, and
covers typed path and query parameters, serde defaults, `Validated<Json<T>>`, delimited lists,
newtypes that validate during deserialisation, a hand-written extractor, a guard that refuses on
policy, an interceptor that answers in place of the handler, and `Validated<Payload<T>>` on
WebSocket and RPC. The file itself carries the NestJS mapping table and a TypeScript snippet beside
each Rust one.

## The rest, by subject

| Subject | Examples |
| --- | --- |
| Getting started | `hello_world`, `provider_patterns`, `derive_injectable` |
| Request lifecycle | `middleware_examples`, `error_handling`, `route_metadata` |
| Extraction | `validation_complete_guide`, `custom_extractors`, `file_upload` |
| Configuration | `config_module`, `config_validation` |
| WebSocket | `websocket_chat`, `websocket_rooms`, `websocket_di`, `gateway_http_bridge` |
| RPC | `rpc_controller`, `rpc_udp`, `rpc_nats`, `rpc_nats_client`, `rpc_tracing` |
| gRPC | `grpc_service` |
| Streaming | `sse` |
| Lifecycle and operations | `lifecycle_hooks`, `graceful_shutdown`, `health_checks`, `logging` |
| Scoping | `request_scoped_context`, `multi_protocol_context` |
| Adapters | `salvo_poc`, `poem_poc`, `rocket_poc` |
| Deployment | `socket_activation` |

`middleware_examples` collects reference implementations — logging, CORS, bearer auth, timeouts,
compression, rate limiting. They illustrate the shape rather than being production-ready.

## Related

- [Architecture decision records](../docs/adr/README.md) — why the framework is built the way it is
- [`validator` crate docs](https://docs.rs/validator/) — the validation attributes `Validated<E>` runs

## License

MIT, same as toni.
