use salvo::websocket::Message;
use toni::websocket::{WsError, WsMessage};

pub(crate) fn salvo_to_ws_message(msg: Message) -> Result<WsMessage, WsError> {
    if msg.is_text() {
        let text = msg.as_str().unwrap_or("").to_string();
        Ok(WsMessage::Text(text))
    } else if msg.is_close() {
        Err(WsError::ConnectionClosed("Close frame received".into()))
    } else if msg.is_ping() {
        Ok(WsMessage::Ping(msg.into()))
    } else if msg.is_pong() {
        Ok(WsMessage::Pong(msg.into()))
    } else if msg.is_binary() {
        Ok(WsMessage::Binary(msg.into()))
    } else {
        Err(WsError::ConnectionClosed("Unknown frame".into()))
    }
}

pub(crate) fn ws_message_to_salvo(msg: WsMessage) -> Result<Message, WsError> {
    match msg {
        WsMessage::Text(text) => Ok(Message::text(text)),
        WsMessage::Binary(data) => Ok(Message::binary(data)),
        WsMessage::Ping(data) => Ok(Message::ping(data)),
        WsMessage::Pong(data) => Ok(Message::pong(data)),
        WsMessage::Close(frame) => Ok(match frame {
            Some(f) => Message::close_with(f.code, f.reason),
            None => Message::close(),
        }),
    }
}
