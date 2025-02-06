use toni_macros::provider_struct;

#[provider_struct(
	pub struct UserService;
)]
impl UserService {
	pub fn find_all(&self) -> String {
		"find_all".to_string()
	}

	pub fn find_by_id(&self, id: i32) -> String {
		format!("find_by_id {}", id)
	}

	pub fn create(&self) -> String {
		"create".to_string()
	}

	pub fn update(&self) -> String {
		"update".to_string()
	}

	pub fn delete(&self) -> String {
		"delete".to_string()
	}
}