use parking_lot::RwLock;
use std::collections::HashMap;
use std::sync::Arc;

use crate::dispatch::transport::Ws;
use crate::error::SetupResult;
use crate::ws::{Gateway, GatewayWrapper};

use super::{Declared, resolve};
use crate::di::internal::Container;

pub(crate) struct GatewayResolver {
    container: Arc<RwLock<Container>>,
}

impl GatewayResolver {
    pub(crate) fn new(container: Arc<RwLock<Container>>) -> Self {
        Self { container }
    }

    pub(crate) fn resolve(&self) -> SetupResult<HashMap<String, Arc<GatewayWrapper>>> {
        let raw = self.container.read().gateways().clone();
        raw.into_iter()
            .map(|(path, gateway)| {
                let wrapper = self.wrap_gateway(gateway)?;
                Ok((path, Arc::new(wrapper)))
            })
            .collect()
    }

    fn wrap_gateway(&self, gateway: Arc<dyn Gateway>) -> SetupResult<GatewayWrapper> {
        let declared = gateway.enhancers();
        let metadata = gateway.metadata();
        let handler_metadata: HashMap<String, Arc<crate::context::Metadata>> =
            gateway.handler_metadata().into_iter().collect();

        let container = self.container.read();
        let registry = &container.role_registry().ws;

        let resolved = resolve::<Ws>(
            registry,
            &container.global_ws,
            Declared {
                guards: declared.guards,
                interceptors: declared.interceptors,
                error_handlers: declared.error_handlers,
            },
            declared.handlers.into_iter().map(|handler| {
                (
                    handler.event,
                    Declared {
                        guards: handler.guards,
                        interceptors: handler.interceptors,
                        error_handlers: handler.error_handlers,
                    },
                )
            }),
        )?;

        Ok(GatewayWrapper::new(
            gateway,
            resolved,
            metadata,
            handler_metadata,
        ))
    }
}
