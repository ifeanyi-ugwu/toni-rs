# The RPC conformance suite against a live Kafka, for a local run: CI runs it,
# with the nats, redis, mqtt and rabbitmq suites, in its `broker integration`
# job on every push to master. Needs Docker.
#
# Each scenario starts a broker of its own. On a host short of memory, set
# ULO_CONFORMANCE_PARALLEL to bound how many hold one at once; the suite takes
# the smaller of it and the broker's own bound, which is 4 for Kafka. CI leaves
# it unset, and so does any host with the memory for the broker's bound:
#
#     ULO_CONFORMANCE_PARALLEL=2 make conformance-kafka
.PHONY: conformance-kafka
conformance-kafka:
	cargo test -p ulo-rpc-kafka --features integration --test conformance --locked
