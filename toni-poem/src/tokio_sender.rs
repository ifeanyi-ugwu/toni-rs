use tokio::sync::mpsc;
use toni::async_trait;
use toni::websocket::{SendError, TrySendError, WsMessage, WsSink};

/// Tokio mpsc-backed `WsSink` used by the poem adapter to forward outbound
/// messages from the framework into the per-connection write task.
#[derive(Clone)]
pub struct TokioSender {
    inner: mpsc::Sender<WsMessage>,
}

impl TokioSender {
    pub fn new(sender: mpsc::Sender<WsMessage>) -> Self {
        Self { inner: sender }
    }
}

#[async_trait]
impl WsSink for TokioSender {
    async fn send(&self, message: WsMessage) -> Result<(), SendError> {
        self.inner.send(message).await.map_err(|_| SendError)
    }

    fn try_send(&self, message: WsMessage) -> Result<(), TrySendError> {
        self.inner.try_send(message).map_err(|e| match e {
            mpsc::error::TrySendError::Full(msg) => TrySendError::Full(msg),
            mpsc::error::TrySendError::Closed(_) => TrySendError::Closed,
        })
    }
}
