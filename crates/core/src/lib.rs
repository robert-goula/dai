//! Docset domain model, ingest, normalization, indexing, and search.

pub mod devdocs;
pub mod index;
pub mod library;
pub mod normalize;
pub mod paths;
pub mod store;

pub use library::{DocPage, Library};
