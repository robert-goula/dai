//! Where MCP tools get their data: the library in-process (inside the daemon)
//! or the daemon over HTTP (the `dai mcp` stdio server).

use std::sync::Arc;

use anyhow::Result;
use dai_core::index::Hit;
use dai_core::store::Docset;
use dai_core::{DocPage, Library};

use crate::client::Client;

#[derive(Clone)]
pub enum Backend {
    Local(Arc<Library>),
    Remote(Client),
}

impl Backend {
    pub async fn docsets(&self) -> Result<Vec<Docset>> {
        match self {
            Self::Local(lib) => blocking(lib, |l| l.installed()).await,
            Self::Remote(c) => c.docsets().await,
        }
    }

    pub async fn search(
        &self,
        query: String,
        docsets: Vec<String>,
        limit: usize,
    ) -> Result<Vec<Hit>> {
        match self {
            Self::Local(lib) => blocking(lib, move |l| l.search(&query, &docsets, limit)).await,
            Self::Remote(c) => c.search(&query, &docsets, limit).await,
        }
    }

    pub async fn get_doc(
        &self,
        docset: String,
        path: String,
        offset: usize,
        max_chars: Option<usize>,
    ) -> Result<Option<DocPage>> {
        match self {
            Self::Local(lib) => {
                blocking(lib, move |l| l.get_doc(&docset, &path, offset, max_chars)).await
            }
            Self::Remote(c) => c.get_doc(&docset, &path, offset, max_chars).await,
        }
    }
}

/// Runs a library call on the blocking pool (SQLite, tantivy, and the DevDocs
/// client are all synchronous).
pub async fn blocking<T, F>(lib: &Arc<Library>, f: F) -> Result<T>
where
    T: Send + 'static,
    F: FnOnce(&Library) -> Result<T> + Send + 'static,
{
    let lib = lib.clone();
    tokio::task::spawn_blocking(move || f(&lib)).await?
}
