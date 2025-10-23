//! # toni-config
//!
//! Configuration management for the Toni framework with type-safe environment variable loading.
//!
//! ## Features
//!
//! - **Declarative configuration** using derive macros
//! - **Automatic environment variable mapping** with sensible defaults
//! - **Type-safe parsing** with helpful error messages
//! - **Validation support** (optional, with `validator` crate)
//! - **Multi-environment support** (.env.development, .env.production, etc.)
//! - **Nested configuration** for complex structures
//!
//! ## Basic Usage
//!
//! ```rust
//! use toni_config::{Config, ConfigModule};
//!
//! #[derive(Config, Clone)]
//! pub struct AppConfig {
//!     #[env("DATABASE_URL")]
//!     pub database_url: String,
//!
//!     #[env("PORT")]
//!     #[default(3000u16)]
//!     pub port: u16,
//!
//!     #[default("127.0.0.1".to_string())]
//!     pub host: String,
//! }
//!
//! # fn main() -> Result<(), Box<dyn std::error::Error>> {
//! // Set environment variables for testing
//! std::env::set_var("DATABASE_URL", "postgres://localhost/mydb");
//! std::env::set_var("PORT", "8080");
//!
//! let config = ConfigModule::<AppConfig>::from_env()?;
//! assert_eq!(config.get().database_url, "postgres://localhost/mydb");
//! assert_eq!(config.get().port, 8080);
//! assert_eq!(config.get().host, "127.0.0.1");
//! # Ok(())
//! # }
//! ```
//!
//! ## Environment Variable Mapping
//!
//! By default, field names are converted to SCREAMING_SNAKE_CASE:
//!
//! ```rust
//! use toni_config::Config;
//!
//! #[derive(Config, Clone)]
//! pub struct AppConfig {
//!     // Uses LOG_LEVEL env var (auto-converted from field name)
//!     #[default("info".to_string())]
//!     pub log_level: String,
//!
//!     // Uses MAX_CONNECTIONS env var
//!     #[default(100usize)]
//!     pub max_connections: usize,
//! }
//! ```
//!
//! ## Optional Fields
//!
//! Use `Option<T>` for optional configuration:
//!
//! ```rust
//! use toni_config::Config;
//!
//! #[derive(Config, Clone)]
//! pub struct AppConfig {
//!     // Optional field - won't error if REDIS_URL is missing
//!     #[env("REDIS_URL")]
//!     pub redis_url: Option<String>,
//! }
//! ```
//!
//! ## Nested Configuration
//!
//! ```rust
//! use toni_config::Config;
//!
//! #[derive(Config, Clone)]
//! pub struct DatabaseConfig {
//!     #[env("DB_HOST")]
//!     #[default("localhost".to_string())]
//!     pub host: String,
//!
//!     #[env("DB_PORT")]
//!     #[default(5432u16)]
//!     pub port: u16,
//! }
//!
//! #[derive(Config, Clone)]
//! pub struct AppConfig {
//!     #[env("PORT")]
//!     #[default(3000u16)]
//!     pub port: u16,
//!
//!     // Nested config - handled automatically
//!     #[nested]
//!     pub database: DatabaseConfig,
//! }
//! ```
//!
//! ## Validation (Optional)
//!
//! Enable the `validation` feature and use `validator` derive:
//!
//! ```toml
//! [dependencies]
//! toni-config = { version = "0.1", features = ["validation"] }
//! validator = { version = "0.20", features = ["derive"] }
//! ```
//!
//! ```rust,ignore
//! use toni_config::Config;
//! use validator::Validate;
//!
//! #[derive(Config, Validate, Clone)]
//! pub struct AppConfig {
//!     #[env("DATABASE_URL")]
//!     #[validate(url)]
//!     pub database_url: String,
//!
//!     #[env("PORT")]
//!     #[default(3000)]
//!     #[validate(range(min = 1, max = 65535))]
//!     pub port: u16,
//! }
//! ```
//!
//! ## Multi-Environment Support
//!
//! ```rust,ignore
//! use toni_config::{ConfigModule, Environment};
//!
//! // Loads from .env.development, .env.production, or .env.test
//! let config = ConfigModule::<AppConfig>::from_env_file(Environment::current())?;
//!
//! // Or explicitly specify:
//! let config = ConfigModule::<AppConfig>::from_env_file(Environment::Production)?;
//! ```

use std::env;
use std::path::PathBuf;

// Re-export the derive macro
pub use toni_macros::Config;

#[cfg(feature = "validation")]
pub use validator;

/// Configuration module that handles loading and validation
pub struct ConfigModule<T> {
    config: T,
}

impl<T: FromEnv + Validate> ConfigModule<T> {
    /// Load configuration from environment variables
    pub fn from_env() -> Result<Self, ConfigError> {
        let config = T::load_from_env()?;
        config.validate()?;
        Ok(Self { config })
    }

    /// Load from .env file(s)
    pub fn from_file(path: impl Into<PathBuf>) -> Result<Self, ConfigError> {
        dotenv::from_path(path.into())?;
        Self::from_env()
    }

    /// Load with environment-specific file
    /// e.g., .env.development, .env.production
    pub fn from_env_file(env: Environment) -> Result<Self, ConfigError> {
        let env_file = match env {
            Environment::Development => ".env.development",
            Environment::Production => ".env.production",
            Environment::Test => ".env.test",
            Environment::Custom(name) => return Self::from_file(format!(".env.{}", name)),
        };

        Self::from_file(env_file)
    }

    /// Get the configuration instance
    pub fn get(&self) -> &T {
        &self.config
    }
}

#[derive(Debug, Clone)]
pub enum Environment {
    Development,
    Production,
    Test,
    Custom(String),
}

impl Environment {
    pub fn from_str(s: &str) -> Self {
        match s.to_lowercase().as_str() {
            "development" | "dev" => Self::Development,
            "production" | "prod" => Self::Production,
            "test" => Self::Test,
            custom => Self::Custom(custom.to_string()),
        }
    }

    pub fn current() -> Self {
        env::var("NODE_ENV")
            .or_else(|_| env::var("APP_ENV"))
            .map(|e| Self::from_str(&e))
            .unwrap_or(Self::Development)
    }
}

/// Trait for configuration validation
pub trait Validate {
    fn validate(&self) -> Result<(), ConfigError>;
}

/// Configuration errors
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("Environment variable '{0}' not found")]
    MissingEnvVar(String),

    #[error("Failed to parse environment variable '{key}': {message}")]
    ParseError { key: String, message: String },

    #[error("Validation failed: {0}")]
    ValidationError(String),

    #[error("Failed to load .env file: {0}")]
    DotenvError(#[from] dotenv::Error),

    #[error("Multiple validation errors: {0:?}")]
    MultipleErrors(Vec<String>),
}

/// Trait for loading configuration from environment
pub trait FromEnv: Sized {
    fn load_from_env() -> Result<Self, ConfigError>;
}
