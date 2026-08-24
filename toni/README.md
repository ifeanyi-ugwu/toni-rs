# Toni Framework

**Toni** is a Rust backend framework designed for building modular and scalable applications inspired by the Nest.js architecture. It provides a structured approach to organizing your code with controllers, services, and modules, while remaining decoupled from the HTTP server (Axum adapted and used by default).

---

## Features

- **Modular Architecture**: Organize your application into reusable modules.
- **HTTP Server Flexibility**: Use Axum or integrate your preferred server.
- **Dependency Injection**: Manage dependencies cleanly with module providers.
- **Macro-Driven Syntax**: Reduce boilerplate with intuitive procedural macros.

---

## Installation

### Prerequisites

- **[Rust & Cargo](https://www.rust-lang.org/tools/install)**: Ensure Rust is installed.
- **Toni CLI**: Install the CLI tool globally:
  ```bash
  cargo install toni-cli
  ```

---

## Quickstart: Build a CRUD App

Use the Toni CLI to create a new project:

```bash
toni new my_app
```

## Project Structure

```
src/
├── app/
│   ├── app.controller.rs
│   ├── app.module.rs
│   ├── app.service.rs
│   └── mod.rs
└── main.rs
```

## Run the Server

```bash
cargo run
```

Test your endpoints at `http://localhost:3000/app`.

---

## Key Concepts

### Project Structure

| File                    | Role                                      |
| ----------------------- | ----------------------------------------- |
| **`app.controller.rs`** | Defines routes and handles HTTP requests. |
| **`app.module.rs`**     | Configures dependencies and module setup. |
| **`app.service.rs`**    | Implements core business logic.           |

### Decoupled HTTP Server

Toni is decoupled from the HTTP server. Adapters exist for Axum (`toni-axum`), Actix-web (`toni-actix`), Poem (`toni-poem`), Salvo (`toni-salvo`), and Rocket (`toni-rocket`); any other server can be integrated by implementing the `HttpAdapter` trait.

## Code Example

**`main.rs`**

```rust
use toni::ToniFactory;
use toni_axum::AxumAdapter;

#[tokio::main]
async fn main() {
    let mut app = ToniFactory::create(AppModule).await.unwrap();
    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();
    app.start().await.unwrap();
}
```

**`app/app.module.rs`** (Root Module)

```rust
#[module(
    imports: [],
    controllers: [AppController],
    providers: [AppService],
    exports: []
)]
pub struct AppModule;
```

**`app/app.controller.rs`** (HTTP Routes)

```rust
#[controller("/app")]
pub struct AppController {
    #[inject]
    app_service: AppService,
}

#[routes]
impl AppController {
    #[post("/")]
    fn create(&self) -> Body {
        Body::text(self.app_service.create())
    }

    #[get("/")]
    fn find_all(&self) -> Body {
        Body::text(self.app_service.find_all())
    }
}
```

**`app/app.service.rs`** (Business Logic)

```rust
#[injectable]
pub struct AppService;

impl AppService {
    pub fn create(&self) -> String {
        "Item created!".into()
    }

    pub fn find_all(&self) -> String {
        "All items!".into()
    }
}
```

---

## License

- **License**: MIT.

---
