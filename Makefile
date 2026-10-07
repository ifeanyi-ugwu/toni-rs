# The RPC conformance suite against a live Kafka, for a local run: CI runs it,
# with the nats, redis, mqtt and rabbitmq suites, in its `rpc conformance
# (brokers)` job on every push to master. Needs Docker.
.PHONY: conformance-kafka
conformance-kafka:
	cargo test -p ulo-rpc-kafka --features integration --test conformance --locked
