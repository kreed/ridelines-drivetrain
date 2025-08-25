pub mod common;

// Include generated API types from build.rs
include!(concat!(env!("OUT_DIR"), "/generated_types.rs"));

// Re-export generated types for convenience
pub mod api {
    pub use super::generated::*;
}
