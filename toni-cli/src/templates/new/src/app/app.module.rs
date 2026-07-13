use toni::*;

use super::app_controller::AppController;
use super::app_service::AppService;

#[module(
  imports: [],
  controllers: [AppController],
  providers: [AppService],
  exports: []
)]
impl AppModule {}
