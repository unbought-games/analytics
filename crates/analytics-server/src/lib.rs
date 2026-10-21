pub mod admin;
pub mod auth;
pub mod bootstrap;
pub mod oidc;
pub mod transport;
pub use analytics_core::{AppState, Extension};
pub use transport::router;
pub static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("../../migrations");
