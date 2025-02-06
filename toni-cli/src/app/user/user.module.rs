use toni_macros::module;

use super::user_controller::*;
use super::user_service::*;

#[module(
  imports: [],
  controllers: [UserController],
  providers: [UserService],
  exports: []
)]
impl UserModule {}