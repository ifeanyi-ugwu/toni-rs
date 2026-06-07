use std::{future::Future, marker::PhantomData};

use toni::DynamicModule;

use crate::client::PrismaClientFactory;

pub struct PrismaModule;

impl PrismaModule {
    /// Register a Prisma client for the entire application.
    ///
    /// `connect` is a closure that produces the generated `PrismaClient`. It is called once
    /// during application startup. The client is registered globally under its concrete type,
    /// so any injectable can declare it as a dependency without additional imports.
    ///
    /// ```ignore
    /// // schema.prisma → cargo prisma generate → generates db::PrismaClient
    /// use toni_prisma::PrismaModule;
    ///
    /// #[module(imports: [PrismaModule::for_root(|| db::new_client())])]
    /// pub struct AppModule;
    /// ```
    ///
    /// Then inject the generated client anywhere:
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct UserService {
    ///     #[inject]
    ///     db: db::PrismaClient,
    /// }
    /// impl UserService {
    ///     pub async fn find_all(&self) -> Vec<db::user::Data> {
    ///         self.db.user().find_many(vec![]).exec().await.unwrap()
    ///     }
    /// }
    /// ```
    pub fn for_root<C, F, Fut>(connect: F) -> DynamicModule
    where
        C: Send + Sync + Clone + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = C> + Send + 'static,
    {
        DynamicModule::builder("PrismaModule")
            .provider(PrismaClientFactory::<C, F, Fut> {
                connect,
                _client: PhantomData,
            })
            .export_token(std::any::type_name::<C>())
            .global()
            .build()
    }
}
