//! HTTP front end for analyze, chop, generate, and render.
//!
//! `sp404-core` does not depend on this crate, and neither does the UI. The
//! server calls `sp404-analyze`, `sp404-song`, and the arrangement renderer.
//! Jobs are stored in memory. Restarting the process drops them.
//!
//! Every route except `GET /health` requires `Authorization: Bearer` matching
//! `SP404_API_KEY`. The process refuses to start when that variable is missing.

#![forbid(unsafe_code)]

mod config;
mod error;
mod pipeline;
mod routes;

pub use config::{config_from_env, Config};
pub use routes::router;
