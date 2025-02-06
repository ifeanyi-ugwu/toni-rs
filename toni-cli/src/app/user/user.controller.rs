use toni_macros::{controller, controller_struct, get, post, put, delete};
use tonirs_core::common_structs::http_helpers::{HttpRequest, Body};
use super::user_service::UserService;

#[controller_struct(
  pub struct UserController {
    user_service: UserService,
  }
)]
#[controller("/user")]
impl UserController {
	#[post("")]
	fn create(&self, req: HttpRequest) -> Box<Body> {
		let create: String = self.user_service.create();
		Box::new(Body::Text(create))
	}

	#[get("")]
	fn find_all(&self, req: HttpRequest) -> Box<Body> {
		let find_all: String = self.user_service.find_all();
		Box::new(Body::Text(find_all))
	}

	#[get("/:id")]
	fn find_by_id(&self, req: HttpRequest) -> Box<Body> {
		let id = req.path_params.get("id").unwrap().parse::<i32>().unwrap();
		let find_by_id: String = self.user_service.find_by_id(id);
		Box::new(Body::Text(find_by_id))
	}

	#[put("")]
	fn update(&self, req: HttpRequest) -> Box<Body> {
		let update: String = self.user_service.update();
		Box::new(Body::Text(update))
	}

	#[delete("")]
	fn delete(&self, req: HttpRequest) -> Box<Body> {
		let delete: String = self.user_service.delete();
		Box::new(Body::Text(delete))
	}
}
