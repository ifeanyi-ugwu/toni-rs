//! The GraphQL-over-HTTP endpoint: a controller mounted `.at(config.path)`, reading
//! `Dep<dyn Engine, Q>`, answering GET and POST by the specification's rules from the response's
//! `Outcome`, and the negotiated media type.
