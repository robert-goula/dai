//! Docset domain model, ingest, normalization, indexing, and search.

pub mod dash;
pub mod devdocs;
pub mod index;
pub mod library;
mod net;
pub mod normalize;
pub mod paths;
pub mod store;

pub use library::{CatalogEntry, DocPage, Library, Progress};
