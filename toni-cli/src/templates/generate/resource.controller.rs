use toni_macros::{controller, controller_struct, get, post, put, delete};
use tonirs_core::common_structs::http_helpers::{HttpRequest, Body};
use super::resource_name_service::RESOURCE_NAME_SERVICE;

#[controller_struct(
  pub struct RESOURCE_NAME_CONTROLLER {
    resource_name_service: RESOURCE_NAME_SERVICE,
  }
)]
#[controller("/resource_name")]
impl RESOURCE_NAME_CONTROLLER {
	#[post("")]
	fn create(&self, req: HttpRequest) -> Body {
		let create: String = self.resource_name_service.create();
		Body::Text(create)
	}

	#[get("")]
	fn find_all(&self, req: HttpRequest) -> Body {
		let find_all: String = self.resource_name_service.find_all();
		Body::Text(find_all)
	}

	#[get("/:id")]
	fn find_by_id(&self, req: HttpRequest) -> Body {
		let id = req.path_params.get("id").unwrap().parse::<i32>().unwrap();
		let find_by_id: String = self.resource_name_service.find_by_id(id);
		Body::Text(find_by_id)
	}

	#[put("")]
	fn update(&self, req: HttpRequest) -> Body {
		let update: String = self.resource_name_service.update();
		Body::Text(update)
	}

	#[delete("")]
	fn delete(&self, req: HttpRequest) -> Body {
		let delete: String = self.resource_name_service.delete();
		Body::Text(delete)
	}
}
