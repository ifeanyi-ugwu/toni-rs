use std::{future::Future, marker::PhantomData};

use ulo::di::{DynamicModule, Key, token_of};

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
    /// use ulo_db_prisma::PrismaModule;
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
    ///
    /// # No startup check
    ///
    /// The other database integrations verify their server answers before the application serves,
    /// and report an unreachable one from startup. This one cannot: `connect` returns the generated
    /// client by value, and there is no operation to call on an arbitrary type to see whether it
    /// works. A client that cannot reach its database fails where it is first used.
    pub fn for_root<C, F, Fut>(connect: F) -> DynamicModule
    where
        C: Send + Sync + Clone + 'static,
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = C> + Send + 'static,
    {
        DynamicModule::builder("PrismaModule")
            .provider(PrismaClientFactory::<C, F, Fut> {
                connect,
                token: ulo::di::token_of::<C>(),
                _client: PhantomData,
            })
            .export::<C>()
            .global()
            .build()
    }

    /// Register a second Prisma client, under the marker `K`.
    ///
    /// `for_root` provides one client injectable by its concrete type. A second one needs a slot of
    /// its own, named by a marker type whose slot holds the client. A marker is required to
    /// register more than one client of the same type: the client is configured by an opaque
    /// `connect` closure, so two `for_root` calls of the same type cannot be told apart
    /// automatically the way a URL-configured connection could.
    ///
    /// ```ignore
    /// key!(pub Analytics: db::PrismaClient);
    ///
    /// #[module(imports: [
    ///     PrismaModule::for_root(|| db::new_client()),
    ///     PrismaModule::for_root_keyed::<Analytics, _>(|| db::new_client_with(analytics_url())),
    /// ])]
    /// pub struct AppModule;
    ///
    /// #[injectable]
    /// pub struct ReportService {
    ///     #[inject(Analytics)]
    ///     db: db::PrismaClient,
    /// }
    /// ```
    ///
    /// The marker names the client across the application. Two clients under one marker are one
    /// module to the container, the client having no configuration to fingerprint: one is
    /// registered and the other dropped as a repeat, within one `imports` list the one written
    /// last, as with two `for_root` calls of one type.
    pub fn for_root_keyed<K, Fut>(
        connect: impl Fn() -> Fut + Send + Sync + 'static,
    ) -> DynamicModule
    where
        K: Key,
        K::Value: Send + Sync + Clone + Sized,
        Fut: Future<Output = K::Value> + Send + 'static,
    {
        DynamicModule::builder(token_of::<K>())
            .provider(PrismaClientFactory {
                connect,
                token: token_of::<K>(),
                _client: PhantomData,
            })
            .export::<K>()
            .global()
            .build()
    }
}
