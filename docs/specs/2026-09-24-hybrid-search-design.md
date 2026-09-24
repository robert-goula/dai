# DAI hybrid (keyword + semantic) search

## Context

Search is BM25 over names, headings, and bodies (tantivy), with stemming and a symbol-aware boost. It's excellent for symbols (`useEffect`, `Vec::pu`) and good for keyword prose. It misses queries phrased differently from the docs: "how do I stop an effect running twice" vs. the docs' "Strict Mode … cleanup". That's the case agents hit most, since they ask in natural language. Local embeddings close that gap without a network dependency.

Goal: better results for prose queries, with no regression for symbols, no network at query time, and a bounded cost in disk, RAM, and install time. It must be optional, and search must work exactly as today when it's off.

## Approach

- **Model.** [fastembed-rs](https://github.com/anush008/fastembed-rs) (ONNX Runtime via `ort`) with a small quantized English model: `BGESmallENV15Q` (384 dims, the crate's default family, tens of MB). It's downloaded on first enable into `<data dir>/models`, not bundled.
- **What gets embedded.** Heading-scoped **chunks** (heading trail + body, truncated to the model's max length) and **snippets** (title, description, first lines of code). Not entries: names are already exact-matched, and embedding short symbols adds noise.
- **Storage.** One flat file per docset, `<data dir>/vectors/<docset>.bin`, holding int8 scalar-quantized vectors (384 bytes each) plus a matching `.ids` file of chunk ids. The files are memory-mapped.
  - **Why not an ANN index (HNSW, usearch):** at DAI's scale (tens to low hundreds of thousands of chunks), a brute-force int8 dot-product scan is on the order of milliseconds. It's exact, has no build step, and deletes are trivial (remove the file). Add ANN only if measurements say so.
  - **Why not SQLite rows:** per-row overhead and no cheap scan.
- **Linking to the index.** Chunks get a stable `chunk_id` (`<docset>:<ordinal>`) stored in tantivy. Vector hits resolve back to chunk documents by that term, so results render exactly like today's hits.
- **When semantic search applies.** Only when:
  1. it's enabled,
  2. the query is prose (contains whitespace and at least 3 words), and
  3. the searched docsets have vectors.

  Symbol and type-ahead queries stay pure BM25: fast, exact, unchanged.
- **Fusion.** Reciprocal Rank Fusion (k = 60) of the BM25 top-50 and the vector top-50, then the existing limit. RRF needs no score calibration between the two systems. Exact entry-name matches keep their current top position.
- **Filters still apply:** docset filters, project version exclusions, and snippet separation apply to the vector side too. Only the permitted docsets' files are scanned.

## Indexing

- **After install/update:** a background job embeds the new docset's chunks at low priority. Search keeps working with BM25 until the vectors are ready.
- **Progress:** a new `install_progress` stage, `embed`, shows in the app, for example "Indexing for semantic search 40%".
- **Changes:** remove deletes the vector files, and update rewrites them.
- **Existing docsets:** enabling the feature queues all installed docsets.
- **Cost to measure first** (the numbers below are estimates to confirm):
  - **Throughput:** a quantized small model on CPU does hundreds to low thousands of chunks per second. A large docset (Rust, ~2,900 pages) would take minutes, not seconds.
  - **Disk:** about 400 bytes per chunk (int8 plus the id), so 100k chunks is about 40MB.
  - **RAM:** the model's working set while embedding. Vector files are memory-mapped, not loaded.

## Configuration

`~/.config/dai/config.toml`:

```toml
[semantic]
enabled = false          # opt-in
model = "bge-small-en-v1.5-q"
```

There's also an app toggle in Settings, which shows the download size and the estimated indexing time for installed docsets.

## Evaluation gate (before building for real)

Build a small benchmark: about 40 prose questions with expected pages, across react, rust, python, and one Dash and one generated docset, plus about 20 symbol queries. Measure MRR@10 and recall@10 for BM25 vs. hybrid, plus p50/p95 latency. Two outcomes:
- **Proceed** only if hybrid clearly beats BM25 on prose and symbols don't regress.
- Otherwise, stop and keep BM25. A cheaper alternative worth measuring in the same harness is static embeddings (model2vec/Potion-style): no ONNX Runtime and much faster indexing, at some cost in quality.

## Risks and trade-offs

- **Binary and dependency weight.** ONNX Runtime adds tens of MB and a native library per platform (prebuilt for macOS, Windows, and Linux on x64 and arm64). That weighs on the installer size and on CI for all three OSes. It's another reason CI should be green first.
- **Indexing time** on big docsets. It's mitigated by running in the background and by being opt-in.
- **English-only model.** Non-English docs (e.g. `Python_zh_cn`) will embed poorly. A multilingual model is possible later at a larger size.
- **Two retrieval paths** to keep consistent: filters, exclusions, and removals must hit both. There are tests for each.

## Phases

1. **H0: Benchmark harness.** Queries, expected pages, and a `dai bench` (or test binary) reporting MRR/recall/latency for BM25. It establishes the baseline and is useful even if hybrid never ships.
2. **H1: Spike.** Embed 2–3 docsets with fastembed, store int8 files, add RRF, and run the harness. **Go/no-go decision here.**
3. **H2: Integrate.** Config and toggle, the background embed job with progress, vector file lifecycle, filters and exclusions on the vector side, and snippets.
4. **H3: Polish.** App settings UI (download size, estimated time), `dai semantic status`, and docs.

## Verification

- Unit tests: int8 quantization round-trip and ordering, RRF merge, chunk-id linkage, filter parity (the vector side never returns excluded docsets), and file cleanup on remove.
- The benchmark harness is the acceptance test: the thresholds agreed at H1 must hold.
- Live: install react with semantic search on, check that the embed progress completes, then ask "how do I stop an effect running twice in development". The Strict Mode/cleanup section should rank in the top 3, while `useEffect` still ranks first for the symbol query.
