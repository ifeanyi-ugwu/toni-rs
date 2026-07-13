use toni::*;

use super::resource_name_controller::RESOURCE_NAME_CONTROLLER;
use super::resource_name_service::RESOURCE_NAME_SERVICE;

#[module(
  imports: [],
  controllers: [RESOURCE_NAME_CONTROLLER],
  providers: [RESOURCE_NAME_SERVICE],
  exports: []
)]
impl RESOURCE_NAME_MODULE {}
