use super::app_service::AppService;
use toni::*;

#[controller("/app")]
pub struct AppController {
    #[inject]
    app_service: AppService,
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
