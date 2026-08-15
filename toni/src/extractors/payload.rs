/// The message payload, deserialised.
///
/// One spelling shared by the transports whose message *is* the payload —
/// WebSocket and RPC. Each supplies its own `FromContext` impl, since what a
/// frame and a call carry differ, and so do the ways they can fail to parse.
///
/// HTTP carries no impl for this. There the payload is the request body, and
/// `Json<T>`, `Bytes`, `Body<T>` and `BodyStream` name it more precisely than one
/// word could.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Payload<T>(pub T);
