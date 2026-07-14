# toni-redis-broadcast

Redis Pub/Sub adapter for cross-process WebSocket broadcasting in the [Toni](https://github.com/monterxto/toni-rs) framework.

## The problem

Toni's built-in `BroadcastService` is in-process. When you run multiple instances of your application behind a load balancer, clients land on different processes. A broadcast call on process A only reaches clients connected to process A — clients on process B never see it.

## How this crate solves it

Every `send()` call publishes the message to a shared Redis Pub/Sub channel. All running instances subscribe to that channel and deliver the message to their locally connected clients. This is the same topology Socket.io uses with `@socket.io/redis-adapter`.

```
Process A                     Redis                      Process B
─────────                  ──────────                  ─────────
rbs.to_room("lobby")  ──►  PUBLISH                ──►  deliver to
  .send(msg)               toni:broadcast               local clients
                                                         in "lobby"
```

## Installation

```toml
[dependencies]
toni = "0.2"
toni-redis-broadcast = "0.1"
```

## Usage

### 1. Import the module

Replace `BroadcastModule::new()` with `RedisBroadcastModule::for_root(url)`. Do not import both.

```rust
use toni_redis_broadcast::RedisBroadcastModule;

#[module(
    imports: [RedisBroadcastModule::for_root("redis://127.0.0.1/")],
    providers: [ChatGateway],
)]
struct AppModule;
```

The module is global — you only need to import it once, in your root module.

### 2. Inject `RedisBroadcastService` where you need it

```rust
use toni_redis_broadcast::RedisBroadcastService;

#[injectable]
struct ChatService {
    #[inject]
    broadcast: RedisBroadcastService,
}
```

### 3. Broadcast

```rust
// Everyone connected across all processes
self.broadcast.to_all().send(msg).await?;

// Everyone in a room
self.broadcast.to_room("lobby").send(msg).await?;

// One specific client (publishes directly to the process holding that client)
self.broadcast.to_client(&client_id).send(msg).await?;

// Everyone except one client
self.broadcast.except(&client_id).send(msg).await?;

// Wrap in {"event": "...", "data": ...} format expected by toni's gateway router
self.broadcast.to_room("lobby").send_event("user.joined", &payload).await?;
```

#### Namespace filtering

```rust
self.broadcast
    .to_room("lobby")
    .in_namespace("chat")
    .send(msg)
    .await?;
```

### 4. Room management

```rust
self.broadcast.join_room(&client_id, "lobby").await?;
self.broadcast.leave_room(&client_id, "lobby").await?;

let rooms = self.broadcast.get_client_rooms(&client_id).await;
let members = self.broadcast.get_room_clients("lobby").await;
```

### Using inside a gateway

```rust
#[websocket_gateway("/chat")]
pub struct ChatGateway {
    #[inject]
    broadcast: RedisBroadcastService,
}

#[subscriptions]
impl ChatGateway {
    #[subscribe_message("join")]
    async fn on_join(&self, client: WsClient, _msg: WsMessage) -> WsHandlerResult {
        self.broadcast.join_room(&client.id, "lobby").await?;
        self.broadcast
            .to_room("lobby")
            .send_event("user.joined", &client.id)
            .await?;
        Ok(WsHandlerOutput::Empty)
    }

    #[subscribe_message("message")]
    async fn on_message(&self, client: WsClient, msg: WsMessage) -> WsHandlerResult {
        let text = msg.as_text().unwrap_or_default().to_string();
        self.broadcast
            .to_room("lobby")
            .send_event("message", &text)
            .await?;
        Ok(WsHandlerOutput::Empty)
    }
}
```

## API reference

### `RedisBroadcastModule`

| Method          | Description                                                        |
| --------------- | ------------------------------------------------------------------ |
| `for_root(url)` | Connect to Redis at the given URL and register the module globally |

### `RedisBroadcastService`

**Broadcasting**

| Method          | Description                                                        |
| --------------- | ------------------------------------------------------------------ |
| `to_all()`      | Target all connected clients across all processes                  |
| `to_room(room)` | Target clients in a room                                           |
| `to_client(id)` | Target one specific client                                         |
| `except(id)`    | Target all clients except one                                      |

All four return a `RedisBroadcastTarget`.

**Room management**

| Method                           | Description                                    |
| -------------------------------- | ---------------------------------------------- |
| `join_room(client_id, room_id)`  | Add a client to a room (synced to Redis)        |
| `leave_room(client_id, room_id)` | Remove a client from a room (synced to Redis)   |
| `get_client_rooms(client_id)`    | List rooms this client is in (global, via Redis) |
| `get_room_clients(room_id)`      | List clients in a room (global, via Redis)      |

### `RedisBroadcastTarget`

| Method                    | Description                                        |
| ------------------------- | -------------------------------------------------- |
| `and_room(room)`          | Chain an additional room target                    |
| `in_namespace(ns)`        | Filter recipients to a namespace                   |
| `send(message)`           | Publish to Redis. Returns `Ok(0)` — see note below |
| `send_event(event, data)` | Publish as `{"event": "...", "data": ...}`         |

> **Note on return value:** `send()` returns `Ok(0)`, not the actual delivery count. Delivery happens asynchronously in each subscriber process and cannot be counted at publish time.

## Running the tests

The integration tests require Docker:

```bash
cargo test -p toni-redis-broadcast -- --ignored
```

On Rancher Desktop the Docker socket is not at the default path, so you need to point testcontainers at it:

```bash
DOCKER_HOST="unix://${HOME}/.rd/docker.sock" cargo test -p toni-redis-broadcast -- --ignored
```
