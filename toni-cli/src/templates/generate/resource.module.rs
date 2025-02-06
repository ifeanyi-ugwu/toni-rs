use toni_macros::module;

use super::resource_name_controller::*;
use super::resource_name_service::*;

#[module(
  imports: [],
  controllers: [RESOURCE_NAME_CONTROLLER],
  providers: [RESOURCE_NAME_SERVICE],
  exports: []
)]
impl RESOURCE_NAME_MODULE {}