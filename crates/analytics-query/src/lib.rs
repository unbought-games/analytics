//! Bounded read-only source adapters and durable, authorized analytical exports.
pub mod safety;

pub mod analytics;
pub mod api;
pub mod execution;
pub mod export;
pub mod http_sources;
pub mod postgres;
pub mod source;
pub use api::QueryExtension;
pub use export::start_worker;
