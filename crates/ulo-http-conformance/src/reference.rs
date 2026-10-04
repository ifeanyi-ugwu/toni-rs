//! The hyper backend as the reference host: the app on `ulo_http_hyper::Server` at port 0, mounted
//! as it would be under a prefix in `Mode::Nested`.

use ulo::App;
use ulo::app::Connected;
use ulo_http::embed::EmbedLimits;

use crate::{Host, Mode};

/// The reference host.
pub struct HyperHost {
    pub(crate) base_url: String,
}

impl Host for HyperHost {
    async fn start(app: App<Connected>, mode: Mode) -> Self {
        let _ = (app, mode);
        todo!()
    }

    fn base_url(&self) -> String {
        self.base_url.clone()
    }

    fn limits() -> EmbedLimits {
        EmbedLimits::NONE
    }

    async fn stop(self) {
        todo!()
    }
}
