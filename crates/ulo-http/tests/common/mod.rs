//! A backend that serves nothing itself, so a test calls `AppService` as a backend would and
//! decides whether a response body is written to its end or dropped before, which is how a peer
//! leaving reaches the execution.

use std::future::poll_fn;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use http_body::Body as _;
use tokio::sync::Notify;
use ulo::BoxError;
use ulo_http::{AppService, Backend, BackendLimits, HttpConfig, Response, StatusCode};
use ulo_net::{BoundListener, TlsAcceptor};

/// Keeps the `AppService` it is handed at `bind`.
#[derive(Clone, Default)]
pub struct Keeper {
    svc: Arc<OnceLock<AppService>>,
    drained: Arc<Notify>,
}

impl Keeper {
    /// The service, once the app has listened.
    pub fn service(&self) -> AppService {
        self.svc.get().cloned().expect("the backend was handed the service")
    }
}

impl Backend for Keeper {
    const NAME: &'static str = "keeper";

    fn limits() -> BackendLimits {
        BackendLimits::NONE
    }

    async fn bind(&mut self, _listeners: Vec<BoundListener>, _tls: Option<TlsAcceptor>, svc: AppService, _cfg: &HttpConfig) -> Result<(), BoxError> {
        let _ = self.svc.set(svc);
        Ok(())
    }

    async fn serve(&self) -> Result<(), BoxError> {
        self.drained.notified().await;
        Ok(())
    }

    async fn drain(&self) {
        self.drained.notify_one();
    }

    async fn close(&self) -> Result<(), BoxError> {
        Ok(())
    }
}

/// The body's frames read to its end, as a backend writing it does; the body then dropped.
pub async fn written(response: Response) -> (StatusCode, Vec<u8>) {
    let (parts, mut body) = response.into_parts();
    let mut data = Vec::new();
    while let Some(frame) = poll_fn(|cx| Pin::new(&mut body).poll_frame(cx)).await {
        if let Ok(bytes) = frame.expect("the body yields no error").into_data() {
            data.extend_from_slice(&bytes);
        }
        if body.is_end_stream() {
            break;
        }
    }
    drop(body);
    (parts.status, data)
}
