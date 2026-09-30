use super::app_service::AppService;
use ulo::http::Body;
use std::sync::Arc;
use ulo::prelude::*;

#[controller("/app")]
pub struct AppController {
    #[inject]
    app_service: Arc<AppService>,
}

#[routes]
impl AppController {
    #[post("/")]
    fn create(&self) -> Body {
        Body::text(self.app_service.create())
    }

    #[get("/")]
    fn find_all(&self) -> Body {
        Body::text(self.app_service.find_all())
    }

    #[put("/")]
    fn update(&self) -> Body {
        Body::text(self.app_service.update())
    }

    #[delete("/")]
    fn delete(&self) -> Body {
        Body::text(self.app_service.delete())
    }
}
