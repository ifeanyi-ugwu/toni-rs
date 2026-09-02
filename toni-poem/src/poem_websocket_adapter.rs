use poem::web::websocket::Message;
use toni::websocket::{WsError, WsMessage};

pub(crate) fn poem_to_ws_message(msg: Message) -> Result<WsMessage, WsError> {
    match msg {
        Message::Text(s) => Ok(WsMessage::Text(s)),
        Message::Binary(b) => Ok(WsMessage::Binary(b)),
        Message::Ping(b) => Ok(WsMessage::Ping(b)),
        Message::Pong(b) => Ok(WsMessage::Pong(b)),
        Message::Close(_) => Err(WsError::ConnectionClosed("Close frame received".into())),
    }
}

pub(crate) fn ws_message_to_poem(msg: WsMessage) -> Result<Message, WsError> {
    match msg {
        WsMessage::Text(s) => Ok(Message::text(s)),
        WsMessage::Binary(b) => Ok(Message::binary(b)),
        WsMessage::Ping(b) => Ok(Message::ping(b)),
        WsMessage::Pong(b) => Ok(Message::pong(b)),
        WsMessage::Close(frame) => Ok(match frame {
            Some(f) => Message::close_with(f.code, f.reason),
            None => Message::close(),
        }),
    }
}
