//! Full-text index (tantivy) over entries and markdown chunks.

use std::path::Path;

use anyhow::Result;
use serde::{Deserialize, Serialize};
use tantivy::collector::TopDocs;
use tantivy::query::{BooleanQuery, BoostQuery, FuzzyTermQuery, Occur, Query, TermQuery};
use tantivy::schema::{
    Field, IndexRecordOption, STORED, STRING, Schema, TextFieldIndexing, TextOptions, Value,
};
use tantivy::snippet::SnippetGenerator;
use tantivy::{IndexReader, IndexWriter, ReloadPolicy, TantivyDocument, Term, doc};

const WRITER_HEAP: usize = 100_000_000;

#[derive(Clone, Copy)]
struct Fields {
    docset: Field,
    kind: Field,
    name: Field,
    name_exact: Field,
    entry_type: Field,
    path: Field,
    title: Field,
    heading: Field,
    body: Field,
}

/// What gets added to the index for one docset.
pub enum IndexDoc<'a> {
    /// A named symbol/page from the docset's own index.
    Entry {
        name: &'a str,
        entry_type: &'a str,
        path: &'a str,
    },
    /// A heading-scoped slice of a page's markdown.
    Chunk {
        path: &'a str,
        title: &'a str,
        heading: &'a str,
        body: &'a str,
    },
    /// A code snippet. Searchable by title, tags, and text.
    Snippet {
        id: &'a str,
        title: &'a str,
        language: &'a str,
        tags: &'a str,
        text: &'a str,
    },
}

/// Pseudo-docset that holds snippets. Doc searches skip it unless it's asked for.
pub const SNIPPETS_DOCSET: &str = "snippets";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hit {
    pub docset: String,
    /// `entry`, `chunk`, or `snippet`.
    pub kind: String,
    /// Entry name, or the page title for chunks.
    pub name: String,
    pub entry_type: String,
    pub path: String,
    pub heading: String,
    pub snippet: String,
    pub score: f32,
}

pub struct Index {
    index: tantivy::Index,
    reader: IndexReader,
    f: Fields,
}

impl Index {
    pub fn open(dir: &Path) -> Result<Self> {
        let (schema, f) = schema();
        std::fs::create_dir_all(dir)?;
        let dir = tantivy::directory::MmapDirectory::open(dir)?;
        let index = tantivy::Index::open_or_create(dir, schema)?;
        let reader = index
            .reader_builder()
            .reload_policy(ReloadPolicy::Manual)
            .try_into()?;
        Ok(Self { index, reader, f })
    }

    /// Replaces everything indexed for `docset` with `docs`.
    pub fn replace_docset<'a>(
        &self,
        docset: &str,
        docs: impl IntoIterator<Item = IndexDoc<'a>>,
    ) -> Result<()> {
        let f = self.f;
        let mut writer: IndexWriter = self.index.writer(WRITER_HEAP)?;
        writer.delete_term(Term::from_field_text(f.docset, docset));
        for d in docs {
            let doc = match d {
                IndexDoc::Entry {
                    name,
                    entry_type,
                    path,
                } => doc!(
                    f.docset => docset, f.kind => "entry", f.name => name,
                    f.name_exact => name.to_lowercase(), f.entry_type => entry_type, f.path => path,
                ),
                IndexDoc::Chunk {
                    path,
                    title,
                    heading,
                    body,
                } => doc!(
                    f.docset => docset, f.kind => "chunk", f.path => path,
                    f.title => title, f.heading => heading, f.body => body,
                ),
                IndexDoc::Snippet {
                    id,
                    title,
                    language,
                    tags,
                    text,
                } => doc!(
                    f.docset => docset, f.kind => "snippet", f.name => title,
                    f.name_exact => title.to_lowercase(), f.entry_type => language,
                    f.path => id, f.heading => tags, f.body => text,
                ),
            };
            writer.add_document(doc)?;
        }
        writer.commit()?;
        self.reader.reload()?;
        Ok(())
    }

    pub fn remove_docset(&self, docset: &str) -> Result<()> {
        self.replace_docset(docset, [])
    }

    /// Ranked search. Exact entry-name matches win, then name terms (with the
    /// last term as a prefix for type-ahead), then headings, then body text.
    /// `docsets` limits the search (empty = all docs); `exclude` drops docsets.
    pub fn search(
        &self,
        query: &str,
        docsets: &[String],
        exclude: &[String],
        limit: usize,
    ) -> Result<Vec<Hit>> {
        let f = self.f;
        let mut should: Vec<(Occur, Box<dyn Query>)> = Vec::new();
        let mut boosted = |q: Box<dyn Query>, boost: f32| {
            should.push((Occur::Should, Box::new(BoostQuery::new(q, boost))))
        };

        let exact = query.trim().to_lowercase();
        if !exact.is_empty() {
            boosted(
                Box::new(TermQuery::new(
                    Term::from_field_text(f.name_exact, &exact),
                    IndexRecordOption::Basic,
                )),
                10.0,
            );
        }
        let name_terms = self.terms(f.name, query)?;
        let is_symbol = !query.trim().contains(char::is_whitespace);
        for (i, t) in name_terms.iter().enumerate() {
            boosted(
                Box::new(TermQuery::new(t.clone(), IndexRecordOption::WithFreqs)),
                4.0,
            );
            // Whitespace-free queries are symbol type-ahead (`usest`, `Vec::pu`): a
            // name prefix on the last term should beat body text (which also matches
            // via stemming, e.g. "usest" ~ "useState"). Prose queries skip this.
            if is_symbol && i + 1 == name_terms.len() {
                boosted(
                    Box::new(FuzzyTermQuery::new_prefix(t.clone(), 0, true)),
                    30.0,
                );
            }
        }
        for t in self.terms(f.heading, query)? {
            boosted(
                Box::new(TermQuery::new(t, IndexRecordOption::WithFreqs)),
                2.0,
            );
        }
        let body_terms = self.terms(f.body, query)?;
        for t in &body_terms {
            boosted(
                Box::new(TermQuery::new(t.clone(), IndexRecordOption::WithFreqs)),
                1.0,
            );
        }
        if should.is_empty() {
            return Ok(Vec::new());
        }

        let mut q: Box<dyn Query> = Box::new(BooleanQuery::new(should));
        if !docsets.is_empty() {
            let filter = docsets
                .iter()
                .map(|d| {
                    let tq: Box<dyn Query> = Box::new(TermQuery::new(
                        Term::from_field_text(f.docset, d),
                        IndexRecordOption::Basic,
                    ));
                    (Occur::Should, tq)
                })
                .collect();
            q = Box::new(BooleanQuery::new(vec![
                (Occur::Must, q),
                (Occur::Must, Box::new(BooleanQuery::new(filter))),
            ]));
        }
        let skip = exclude
            .iter()
            .map(String::as_str)
            .chain(docsets.is_empty().then_some(SNIPPETS_DOCSET));
        let mut clauses = vec![(Occur::Must, q)];
        for d in skip {
            let tq: Box<dyn Query> = Box::new(TermQuery::new(
                Term::from_field_text(f.docset, d),
                IndexRecordOption::Basic,
            ));
            clauses.push((Occur::MustNot, tq));
        }
        let q: Box<dyn Query> = Box::new(BooleanQuery::new(clauses));

        let searcher = self.reader.searcher();
        let top = searcher.search(&q, &TopDocs::with_limit(limit).order_by_score())?;
        let body_query = BooleanQuery::new(
            body_terms
                .into_iter()
                .map(|t| {
                    let tq: Box<dyn Query> =
                        Box::new(TermQuery::new(t, IndexRecordOption::WithFreqs));
                    (Occur::Should, tq)
                })
                .collect(),
        );
        let mut snippets = SnippetGenerator::create(&searcher, &body_query, f.body)?;
        snippets.set_max_num_chars(240);

        top.into_iter()
            .map(|(score, addr)| {
                let d: TantivyDocument = searcher.doc(addr)?;
                let get = |field| {
                    d.get_first(field)
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string()
                };
                let kind = get(f.kind);
                let snippet = if kind == "chunk" {
                    snippets.snippet_from_doc(&d).fragment().to_string()
                } else {
                    String::new()
                };
                Ok(Hit {
                    docset: get(f.docset),
                    name: if kind == "chunk" {
                        get(f.title)
                    } else {
                        get(f.name)
                    },
                    kind,
                    entry_type: get(f.entry_type),
                    path: get(f.path),
                    heading: get(f.heading),
                    snippet,
                    score,
                })
            })
            .collect()
    }

    /// Runs `text` through `field`'s tokenizer so query terms match indexed terms.
    fn terms(&self, field: Field, text: &str) -> Result<Vec<Term>> {
        let mut tokenizer = self.index.tokenizer_for_field(field)?;
        let mut stream = tokenizer.token_stream(text);
        let mut out = Vec::new();
        while stream.advance() {
            out.push(Term::from_field_text(field, &stream.token().text));
        }
        Ok(out)
    }
}

fn schema() -> (Schema, Fields) {
    let text = |tokenizer: &str| {
        TextOptions::default().set_stored().set_indexing_options(
            TextFieldIndexing::default()
                .set_tokenizer(tokenizer)
                .set_index_option(IndexRecordOption::WithFreqsAndPositions),
        )
    };
    let mut b = Schema::builder();
    let f = Fields {
        docset: b.add_text_field("docset", STRING | STORED),
        kind: b.add_text_field("kind", STRING | STORED),
        name: b.add_text_field("name", text("default")),
        name_exact: b.add_text_field("name_exact", STRING),
        entry_type: b.add_text_field("type", STORED),
        path: b.add_text_field("path", STORED),
        title: b.add_text_field("title", STORED),
        heading: b.add_text_field("heading", text("en_stem")),
        body: b.add_text_field("body", text("en_stem")),
    };
    (b.build(), f)
}
