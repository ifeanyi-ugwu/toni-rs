//! File upload via multipart/form-data
//!
//! Demonstrates the `Multipart` extractor for handling file uploads and mixed
//! form fields in a single request.
//!
//! Run with:  cargo run --example file_upload
//!
//! Test with curl:
//!
//!   # Upload a file
//!   curl -X POST http://127.0.0.1:3000/upload \
//!        -F "description=my photo" \
//!        -F "file=@/path/to/photo.jpg"
//!
//!   # Inspect a multipart request (all fields echoed back as JSON)
//!   curl -X POST http://127.0.0.1:3000/inspect \
//!        -F "username=alice" \
//!        -F "avatar=@/path/to/avatar.png"

use serde_json::{json, Value};
use toni::extractors::Multipart;
use toni::*;
use toni_axum::AxumAdapter;

#[controller("/")]
pub struct UploadController;

#[routes]
impl UploadController {
    /// Accepts a multipart form with a `file` field and an optional
    /// `description` text field.
    #[post("/upload")]
    async fn upload(&self, mut mp: Multipart) -> Body {
        let mut file_name = String::from("unnamed");
        let mut file_size = 0usize;
        let mut description = String::new();

        while let Some(field) = mp.next_field().await.unwrap() {
            match field.name() {
                Some("description") => {
                    description = field.text().await.unwrap_or_default();
                }
                Some("file") => {
                    file_name = field.file_name().unwrap_or("unnamed").to_string();
                    let data = field.bytes().await.unwrap_or_default();
                    file_size = data.len();
                    println!("📁 received file: {file_name} ({file_size} bytes)");
                }
                _ => {}
            }
        }

        Body::json(json!({
            "file": file_name,
            "size": file_size,
            "description": description,
        }))
    }

    /// Echoes back every field name, content-type, and size.
    #[post("/inspect")]
    async fn inspect(&self, mut mp: Multipart) -> Body {
        let mut fields: Vec<Value> = Vec::new();

        while let Some(field) = mp.next_field().await.unwrap() {
            let name = field.name().unwrap_or("unknown").to_string();
            let content_type = field
                .content_type()
                .map(|m| m.to_string())
                .unwrap_or_default();
            let file_name = field.file_name().map(str::to_string);
            let data = field.bytes().await.unwrap_or_default();

            fields.push(json!({
                "name": name,
                "content_type": content_type,
                "file_name": file_name,
                "size": data.len(),
            }));
        }

        Body::json(json!({ "fields": fields }))
    }
}

#[module(controllers: [UploadController], providers: [])]
impl AppModule {}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("🚀 toni file upload example\n");
    println!("  POST http://127.0.0.1:3000/upload   — upload a file field + description");
    println!("  POST http://127.0.0.1:3000/inspect  — echo back all multipart fields");
    println!();
    println!("  curl -X POST http://127.0.0.1:3000/upload \\");
    println!("       -F \"description=hello\" -F \"file=@/path/to/file.txt\"");
    println!();

    let mut app = ToniFactory::new().create_with(AppModule).await;

    app.use_http_adapter(AxumAdapter::new(), 3000, "127.0.0.1")
        .unwrap();

    app.start().await?;
    Ok(())
}
