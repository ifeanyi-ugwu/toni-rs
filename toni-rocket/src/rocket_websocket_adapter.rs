use rocket_ws::Message;
use toni::websocket::{WsError, WsMessage};

pub(crate) fn rocket_to_ws_message(msg: Message) -> Result<WsMessage, WsError> {
    match msg {
        Message::Text(s) => Ok(WsMessage::Text(s.to_string())),
        Message::Binary(b) => Ok(WsMessage::Binary(b.to_vec())),
        Message::Ping(b) => Ok(WsMessage::Ping(b.to_vec())),
        Message::Pong(b) => Ok(WsMessage::Pong(b.to_vec())),
        Message::Close(_) => Err(WsError::ConnectionClosed("Close frame received".into())),
        Message::Frame(_) => Err(WsError::ConnectionClosed("Raw frame received".into())),
    }
}

pub(crate) fn ws_message_to_rocket(msg: WsMessage) -> Result<Message, WsError> {
    match msg {
        WsMessage::Text(s) => Ok(Message::Text(s.into())),
        WsMessage::Binary(b) => Ok(Message::Binary(b.into())),
        WsMessage::Ping(b) => Ok(Message::Ping(b.into())),
        WsMessage::Pong(b) => Ok(Message::Pong(b.into())),
        WsMessage::Close => Ok(Message::Close(None)),
    }
}
