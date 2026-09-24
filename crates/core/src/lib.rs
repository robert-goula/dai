//! Docset domain model, ingest, normalization, indexing, and search.

pub mod dash;
pub mod devdocs;
pub mod generate;
pub mod index;
pub mod library;
pub mod markdown;
mod net;
pub mod normalize;
pub mod paths;
pub mod project;
pub mod snippets;
pub mod store;

pub use library::{CatalogEntry, DocPage, Library, Progress};
