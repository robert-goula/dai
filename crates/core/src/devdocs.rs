//! DevDocs catalog and document downloads (https://devdocs.io).

use std::collections::HashMap;
use std::time::Duration;

use anyhow::Result;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::store::Entry;

const CATALOG_URL: &str = "https://devdocs.io/docs.json";
const DOCS_BASE: &str = "https://documents.devdocs.io";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogDoc {
    pub name: String,
    pub slug: String,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub release: String,
    pub mtime: i64,
    #[serde(default)]
    pub db_size: u64,
}

#[derive(Debug, Deserialize)]
pub struct DocIndex {
    pub entries: Vec<Entry>,
}

/// Page path -> page HTML.
pub type DocDb = HashMap<String, String>;

pub struct Client {
    http: reqwest::blocking::Client,
}

impl Client {
    pub fn new() -> Result<Self> {
        Ok(Self {
            http: crate::net::client(Some(Duration::from_secs(600)))?,
        })
    }

    pub fn catalog(&self) -> Result<Vec<CatalogDoc>> {
        self.get_json(CATALOG_URL)
    }

    pub fn index(&self, doc: &CatalogDoc) -> Result<DocIndex> {
        self.get_json(&format!(
            "{DOCS_BASE}/{}/index.json?{}",
            doc.slug, doc.mtime
        ))
    }

    pub fn db(&self, doc: &CatalogDoc) -> Result<DocDb> {
        self.get_json(&format!("{DOCS_BASE}/{}/db.json?{}", doc.slug, doc.mtime))
    }

    fn get_json<T: DeserializeOwned>(&self, url: &str) -> Result<T> {
        let bytes = self.http.get(url).send()?.error_for_status()?.bytes()?;
        Ok(serde_json::from_slice(&bytes)?)
    }
}

pub fn page_url(slug: &str, path: &str) -> String {
    format!("https://devdocs.io/{slug}/{path}")
}
