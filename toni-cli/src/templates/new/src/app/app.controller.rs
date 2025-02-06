use toni_macros::{controller, controller_struct, get, post, put, delete};
use tonirs_core::common_structs::http_helpers::{HttpRequest, Body};
use super::app_service::AppService;

#[controller_struct(
  pub struct AppController {
    app_service: AppService,
  }
)]
#[controller("/app")]
impl AppController {
	#[post("")]
	fn create(&self, req: HttpRequest) -> Body {
		let create: String = self.app_service.create();
		Body::Text(create)
	}

	#[get("")]
	fn find_all(&self, req: HttpRequest) -> Body {
		let find_all: String = self.app_service.find_all();
		Body::Text(find_all)
	}

	#[put("")]
	fn update(&self, req: HttpRequest) -> Body {
		let update: String = self.app_service.update();
		Body::Text(update)
	}

	#[delete("")]
	fn delete(&self, req: HttpRequest) -> Body {
		let delete: String = self.app_service.delete();
		Body::Text(delete)
	}
}
