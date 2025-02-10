use std::{cell::RefCell, rc::Rc};

use anyhow::Result;

use crate::{
    http_adapter::HttpAdapter,
    injector::ToniContainer, router::RoutesResolver,
};

pub struct ToniApplication<H: HttpAdapter> {
    http_adapter: H,
    routes_resolver: RoutesResolver,
}

impl<H: HttpAdapter> ToniApplication<H> {
    pub fn new(http_adapter: H, container: Rc<RefCell<ToniContainer>>) -> Self {
        Self {
            http_adapter,
            routes_resolver: RoutesResolver::new(container.clone()),
        }
    }

    pub fn init(&mut self) -> Result<()> {
        self.routes_resolver.resolve(&mut self.http_adapter)?;
        Ok(())
    }
    pub async fn listen(self, port: u16, hostname: &str) -> Result<()> {
        self.http_adapter.listen(port, hostname).await?;
        Ok(())
    }
}
