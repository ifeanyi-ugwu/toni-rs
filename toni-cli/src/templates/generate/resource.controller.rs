use super::resource_name_service::RESOURCE_NAME_SERVICE;
use toni::extractors::Path;
use toni::*;

#[controller("/resource_name")]
pub struct RESOURCE_NAME_CONTROLLER {
    #[inject]
    resource_name_service: RESOURCE_NAME_SERVICE,
}

#[routes]
impl RESOURCE_NAME_CONTROLLER {
    #[post("/")]
    fn create(&self) -> Body {
        Body::text(self.resource_name_service.create())
    }

    #[get("/")]
    fn find_all(&self) -> Body {
        Body::text(self.resource_name_service.find_all())
    }

    #[get("/{id}")]
    fn find_by_id(&self, Path(id): Path<String>) -> Body {
        Body::text(self.resource_name_service.find_by_id(id))
    }

    #[put("/")]
    fn update(&self) -> Body {
        Body::text(self.resource_name_service.update())
    }

    #[delete("/")]
    fn delete(&self) -> Body {
        Body::text(self.resource_name_service.delete())
    }
}
