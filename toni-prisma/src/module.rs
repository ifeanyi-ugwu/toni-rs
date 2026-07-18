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
                token: std::any::type_name::<C>().to_string(),
                _client: PhantomData,
            })
            .export_token(std::any::type_name::<C>())
            .global()
            .build()
    }

    /// Register a second, named Prisma client.
    ///
    /// `for_root` provides one client injectable by its concrete type. When an application needs
    /// more than one client, each additional one is registered under a name and injected by that
    /// name — the type alone can no longer tell them apart.
    ///
    /// A name is required to register more than one client of the same type: the client is
    /// configured by an opaque `connect` closure, so two `for_root` calls of the same type cannot
    /// be told apart automatically the way a URL-configured connection could.
    ///
    /// ```ignore
    /// #[module(imports: [
    ///     PrismaModule::for_root(|| db::new_client()),
    ///     PrismaModule::for_root_named("analytics", || db::new_client_with(analytics_url())),
    /// ])]
    /// pub struct AppModule;
    /// ```
    ///
    /// ```ignore
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject("analytics")]
    ///     db: db::PrismaClient,
    /// }
    /// ```
    ///
    /// The name is a global identifier: two clients cannot share one, and reusing a name across
    /// integrations is refused at startup.
    pub fn for_root_named<C, F, Fut>(name: impl Into<String>, connect: F) -> DynamicModule
    where
        C: Send + Sync + Clone + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = C> + Send + 'static,
    {
        let name: String = name.into();
        DynamicModule::builder(format!("PrismaModule::{name}"))
            .provider(PrismaClientFactory::<C, F, Fut> {
                connect,
                token: name.clone(),
                _client: PhantomData,
            })
            .export_token(name)
            .global()
            .build()
    }
}
