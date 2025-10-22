use std::fmt::Debug;

use validator::ValidationErrors;

pub trait Validatable: Send + Sync + Debug {
    fn validate_dto(&self) -> Result<(), ValidationErrors>;
}
