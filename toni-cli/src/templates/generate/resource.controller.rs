use toni_macros::{controller, controller_struct, get, post, put, delete};
use toni_core::http_helpers::{HttpRequest, Body};
use super::resource_name_service::_RESOURCE_NAME_SERVICE;

#[controller_struct(
  pub struct _RESOURCE_NAME_CONTROLLER {
    resource_name_service: _RESOURCE_NAME_SERVICE,
  }
)]
#[controller("/resource_name")]
impl _RESOURCE_NAME_CONTROLLER {
	#[post("")]
	fn _create(&self, _req: HttpRequest) -> Body {
		let create: String = self.resource_name_service.create();
		Body::Text(create)
	}

	#[get("")]
	fn _find_all(&self, _req: HttpRequest) -> Body {
		let find_all: String = self.resource_name_service.find_all();
		Body::Text(find_all)
	}

	#[get("/{id}")]
	fn _find_by_id(&self, req: HttpRequest) -> Body {
		let id = req.path_params.get("id").unwrap().parse::<i32>().unwrap();
		let find_by_id: String = self.resource_name_service.find_by_id(id);
		Body::Text(find_by_id)
	}

	#[put("")]
	fn _update(&self, _req: HttpRequest) -> Body {
		let update: String = self.resource_name_service.update();
		Body::Text(update)
	}

	#[delete("")]
	fn _delete(&self, _req: HttpRequest) -> Body {
		let delete: String = self.resource_name_service.delete();
		Body::Text(delete)
	}
}
