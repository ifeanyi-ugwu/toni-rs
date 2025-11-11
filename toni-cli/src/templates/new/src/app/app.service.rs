use toni_macros::injectable;

#[injectable(
	pub struct _AppService;
)]
impl _AppService {
    pub fn find_all(&self) -> String {
        "find_all".to_string()
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
