use toni_macros::module;

use super::app_controller::*;
use super::app_service::*;

#[module(
  imports: [],
  controllers: [AppController],
  providers: [AppService],
  exports: []
)]
impl AppModule {}