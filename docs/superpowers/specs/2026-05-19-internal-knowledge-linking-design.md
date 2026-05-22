# Internal Knowledge-Base Cross-Linking — Design Proposal

> Status: PROPOSAL (read-only analysis, nothing modified)
> Date: 2026-05-19
> Scope: attune OSS, Rust line (`rust/crates/attune-core` + `attune-server`)
> Question answered: 知识库内部内容的内联如何建立 — how to establish links between
> related items inside a user's knowledge base, building a knowledge graph over the vault.

---

## 0. Executive summary

attune already has every ingredient for internal linking but **none of them are wired
into a persistent, surfaced link layer**:

- `entity_graph.rs` (`EntityNode` / `EntityEdge` / `build_from_items`) — fully
  implemented, **declared in `lib.rs:102` and used by 0 callers**. In-memory + JSON
  only, never persisted, never built at ingest time.
- `entities.rs` + `skills/extract_entities.rs` — two parallel entity extractors
  (see §6, a real defect to resolve).
- `memory/semantic.rs` — L3 already clusters episodic memories by topic via
  `hdbscan`. That is *topic clustering of conversations*, not *item-to-item links*.
- `search.rs::search_with_context` — hybrid RRF + rerank; vector index
  (`vectors.rs`) gives semantic neighbours for free.
- No `item_relations` / `links` table. No `related` route. `KnowledgeView.tsx` is a
  Phase-4 `EmptyState` placeholder. `Reader.tsx` has no related-items panel.

The proposal: introduce an **`item_links` table** and a **`links` module** in
`attune-core` that computes links **at ingest time, entirely in the 🆓/⚡ cost tier**
(CPU + the embedding already produced — *no per-link LLM*), then surface them in the
Reader, in search/RAG, and in the panorama graph. This is gap-analysis item #13
("Entity graph", OSS, ~2 day) made concrete and given a storage + RAG + UI design.

---

## 1. What gets linked, and on what basis

### 1.1 Granularity decision: link **items**, key on **chunks**

Links are between **items** (the `items` table row — the user-meaningful unit shown
in `ItemsView` / Reader / panorama). Chunk-to-chunk linking would explode O(n²) and
has no UI surface. But the *evidence* for a link is collected at chunk granularity
(an entity occurs in a chunk; a vector neighbour is a chunk) and then **rolled up to
item pairs**. So: compute per-chunk, store per-item-pair, optionally keep the best
chunk anchor on the link row for deep-linking into the Reader.

### 1.2 Recommended link types (all cheap-tier computable)

| `relation` | Basis | Compute input | Cost tier |
|---|---|---|---|
| `shared_entity` | Two items mention the same person/org/place/date/money entity | `entities::extract_entities` output, Jaccard-style overlap | 🆓 CPU + regex |
| `semantic_near` | Two items are vector-space neighbours above a cosine threshold | existing `VectorIndex` chunk vectors (already built by embed worker) | ⚡ reuse existing embeddings, 0 new model calls |
| `shared_tag` | Two items share a high-signal classification tag (domain/topic) | `TagIndex` forward index | 🆓 CPU, hashmap |
| `explicit_ref` | One item's content literally cites another (URL match, `[[wikilink]]`, file path, item title substring) | text scan at ingest | 🆓 CPU + regex |
| `same_cluster` | Two items fall in the same hdbscan cluster (panorama) | `clusterer.rs::rebuild` labels | ⚡ clustering already runs for the panorama |

Recommendation for v1: ship **`shared_entity` + `semantic_near` + `explicit_ref`**
as the three load-bearing types. `shared_tag` and `same_cluster` are derivable from
already-persisted data (`TagIndex`, `ClusterSnapshot`) and can be computed on read
without a stored row — keep them as **derived/virtual links** (§2.4) to avoid table
bloat. Each stored link carries a `weight: f32` and `relation: TEXT`, mirroring the
already-designed `EntityEdge` shape in `entity_graph.rs`.

**Why these three are the right primitives** — they are orthogonal: `explicit_ref`
is precise but sparse, `shared_entity` is structured/explainable ("both mention 张三
+ ¥500,000"), `semantic_near` is fuzzy/recall-heavy. Together they cover
precision→recall the same way the RRF stack covers vector+FTS.

### 1.3 Directionality

`shared_entity` / `semantic_near` / `same_cluster` are **symmetric** — store one row
with canonical ordering (`item_a < item_b` lexicographically, the same convention
`entity_graph.rs::build_from_items` already uses for `from < to`). `explicit_ref` is
**directed** (A cites B ≠ B cites A) — store with `a = citing`, `b = cited`, and a
`directed INTEGER` flag so the read layer knows not to symmetrize it.

---

## 2. Compute + storage design

### 2.1 New table `item_links` (additive, `CREATE TABLE IF NOT EXISTS`)

Follows the existing schema convention in `store/mod.rs` (every table is
`CREATE TABLE IF NOT EXISTS` so old vaults auto-migrate — see the `item_blobs`
comment at `mod.rs:77`). No encryption needed on the structural columns; the
`evidence` text is a short machine string ("张三, ¥500000") not user content, but to
stay conservative keep it minimal / entity-id-only and treat it as low-sensitivity.

```sql
CREATE TABLE IF NOT EXISTS item_links (
    item_a     TEXT NOT NULL,          -- canonical: item_a < item_b for symmetric kinds
    item_b     TEXT NOT NULL,
    relation   TEXT NOT NULL,          -- shared_entity | semantic_near | explicit_ref
    weight     REAL NOT NULL,          -- overlap count / cosine / 1.0
    directed   INTEGER NOT NULL DEFAULT 0,
    evidence   TEXT NOT NULL DEFAULT '',  -- e.g. "person:张三;money:¥500000" or matched URL
    anchor_chunk INTEGER,              -- best chunk_idx in item_a for Reader deep-link
    updated_at TEXT NOT NULL,
    PRIMARY KEY (item_a, item_b, relation)
);
CREATE INDEX IF NOT EXISTS idx_item_links_a ON item_links(item_a, weight DESC);
CREATE INDEX IF NOT EXISTS idx_item_links_b ON item_links(item_b, weight DESC);
```

The two indexes make "related items for X" an O(log n) lookup from either side.

### 2.2 New table `item_entities` — the cross-item entity index

`entity_graph.rs` builds the graph from an in-memory `Vec<(item_id, Vec<Entity>)>`.
For incremental ingest we need entities **persisted per item** so a new item can be
linked against the existing corpus without re-reading every other item's content.

```sql
CREATE TABLE IF NOT EXISTS item_entities (
    item_id    TEXT NOT NULL,
    kind       TEXT NOT NULL,          -- person | organization | date | money | location
    value      TEXT NOT NULL,          -- normalized entity string
    occurrences INTEGER NOT NULL DEFAULT 1,
    PRIMARY KEY (item_id, kind, value)
);
CREATE INDEX IF NOT EXISTS idx_item_entities_kv ON item_entities(kind, value);
```

`idx_item_entities_kv` is the **inverted index** that makes `shared_entity` linking
cheap: for each entity in a freshly-ingested item, `SELECT item_id FROM item_entities
WHERE kind=? AND value=?` returns every other item already mentioning it — no scan.
This table also directly powers the panorama entity graph (it *is*
`EntityNode.item_ids` materialized) and lets `attune-pro` verticals query "all items
mentioning case 张三 v. 李四" without owning their own index.

### 2.3 New module `attune-core/src/links.rs`

Mirror the structure of `reindex.rs` (transactional, multi-resource, one high-level
API so downstream paths can't each forget a step). Public surface:

```rust
/// Re-extract entities for one item and refresh its links against the corpus.
/// Called from the reindex pipeline after content is committed.
pub fn relink_item(
    store: &Store, dek: &Key32,
    vectors: &VectorIndex,
    tag_index: &TagIndex,        // for shared_tag (derived; optional)
    item_id: &str, title: &str, content: &str,
) -> Result<RelinkStats>;

/// Drop all links + entities for a deleted item (delete path).
pub fn purge_item_links(store: &Store, item_id: &str) -> Result<usize>;
```

`relink_item` steps (all 🆓/⚡):

1. `entities::extract_entities(content)` → upsert into `item_entities`
   (delete-then-insert this item's rows, same pattern as `TagIndex::upsert`).
2. **shared_entity**: for each `(kind,value)`, query `item_entities` inverted index
   for co-mentioning items; accumulate per-other-item overlap count → `weight`.
   Apply a floor (e.g. ≥2 shared entities, or ≥1 shared non-trivial entity such as
   a person/org — dates alone are too noisy) before writing a row. Drop generic
   entities by document frequency (an entity in >30 % of items is a stop-entity).
3. **semantic_near**: take this item's chunk vectors from `VectorIndex`
   (`get_vector` per chunk, or run `VectorIndex::search` with each chunk vector),
   collect neighbouring `VectorMeta.item_id`s above `cosine ≥ ~0.82`, roll up to the
   max cosine per other-item → `weight`. **Zero new embedding calls** — the embed
   worker already produced these vectors; this is pure HNSW lookup (⚡, sub-ms).
4. **explicit_ref**: regex/scan `content` for (a) URLs matching another item's
   `url`, (b) `[[title]]` wiki-style refs, (c) verbatim other-item titles ≥ N chars.
   Directed rows.
5. `DELETE FROM item_links WHERE item_a=?1 OR item_b=?1` for this item, then
   `INSERT` the freshly computed set (idempotent rebuild, same philosophy as
   `reindex_item` "清旧 → 加新"). Cap out-degree (e.g. top-20 by weight per item) so
   the graph stays sparse and the panorama stays renderable.

### 2.4 Where it runs — ingest-time, in the embed worker, NOT a route handler

The cost contract (CLAUDE.md §"成本感知与触发契约") says build-phase work must stop
at "searchable + 150-char archive summary" and must not block on the third tier.
Linking is 🆓/⚡, so it belongs in the **build phase** — but it must run *after*
embeddings exist (semantic_near needs vectors). Two clean hook points:

- **Primary**: extend the embed-queue worker. When the worker finishes the last
  chunk of an item (queue for that `item_id` drains), enqueue a `relink` job (reuse
  the `reindex_queue` table mechanism, or add a `link_queue`). The worker thread
  holds no server lock issues because it goes through `Store` + a borrowed
  `VectorIndex` the same way the reindex worker does (CLAUDE.md "Lock ordering":
  `vault → vectors → fulltext → embedding`; `links` only needs `vault + vectors`,
  a strict prefix — safe).
- **Alternative for shared_entity / explicit_ref only** (no vectors needed): compute
  inside `reindex.rs::reindex_item` right after `enqueue_embedding`, since those two
  link types need only `content`. `semantic_near` then runs as a deferred pass when
  vectors land. This split keeps the precise links instant and the fuzzy ones
  eventually-consistent.

**Hard rule: no per-link LLM call.** Relationship *labels* stay mechanical
(`shared_entity` / `semantic_near` / `explicit_ref`) — exactly like law-pro's
`evidence_chain` emits mechanical `RelationKind::SameParty` / `SameAmount` /
`Corroborates` with a descriptive `detail` string and explicitly "无 verdict". Any
LLM-authored relationship explanation is a **third-tier, user-triggered** action
(§3.4), never part of build.

### 2.5 Derived (virtual) links — `shared_tag`, `same_cluster`

Not stored. Computed on read from `TagIndex` (already in memory in `AppState`) and
the latest `ClusterSnapshot`. The `related` route (§3.1) merges stored + derived
links into one response. This keeps `item_links` small and avoids a write fan-out
every time a tag or cluster changes.

---

## 3. Surfacing — where the user sees links

### 3.1 New route `GET /api/v1/items/{id}/related`

attune currently has **no related-items endpoint** (`routes/items.rs` has get /
update / delete / stats / privacy only; `folder_links.rs` is unrelated — it's
filesystem folders). Add `related_items` to `routes/items.rs`:

```
GET /api/v1/items/{id}/related?limit=10
→ { "related": [
     { "item_id", "title", "relation", "weight", "evidence", "anchor_chunk" },
     ... ] }
```

Implementation: union of `item_links` rows (both `item_a` and `item_b` sides) +
derived `shared_tag` / `same_cluster`, sorted by `weight`, joined to `items` for
title. Pure SQL + in-memory merge — 🆓.

### 3.2 Reader modal — "Related" panel

`Reader.tsx` (the item-detail modal) gains a **Related** section below the content:
a short list of linked items grouped by `relation` ("Mentions the same people",
"Similar topic", "References this"), each clickable to open that item's Reader.
`anchor_chunk` lets a click jump to the relevant section (the Reader already has
chunk/section structure via `extract_sections`). This is the highest-value surface —
it turns every document into a hub.

### 3.3 Search results — link-aware ranking + "also related" chips

Two uses in the `search.rs` stack:

- **RAG expansion (the important one)**: in `search_with_context`, after RRF +
  rerank produce the top-K item set, do **one bounded graph hop** — pull
  `item_links` neighbours of the top results with `weight` above a threshold and
  `relation IN (shared_entity, explicit_ref)`, add them as low-priority candidates,
  re-score. A query that hits item A ("the contract") then also surfaces linked item
  B ("the bank statement that corroborates it") even if B's text didn't match the
  query terms. This is the RAG strengthening the brief asks for. Gate it behind a
  `SearchParams` flag (`link_expansion: bool`, default on for `with_defaults_for_rag`,
  off for the Chrome-extension `search_relevant` to keep that path lean). Budget:
  cap added candidates (e.g. ≤3) and keep them below direct hits in
  `allocate_budget` so they enrich rather than displace.
- **UI search list**: each result row can show a small "+N related" chip; clicking
  expands the linked items inline. Cheap — reuse the `related` route.

### 3.4 Knowledge panorama — the graph view

`KnowledgeView.tsx` is currently an `EmptyState` placeholder; the panorama referred
to in the brief is the existing hdbscan **cluster** 2D view. The link layer upgrades
it from *clusters (表象)* to *graph (结构)* — exactly the v07 gap-analysis note
("cluster 是表象, graph 是结构"). Add `GET /api/v1/knowledge/graph`:

```
→ EntityGraph.to_json()  // nodes + edges, the format entity_graph.rs ALREADY emits
```

Build it server-side from `item_entities` + `item_links` (or directly via
`entity_graph::build_from_items` fed from `item_entities`). The frontend renders a
force-directed graph: item nodes + entity nodes, edges colored by `relation`. The
optional third-tier action lives here and in the Reader: a user clicking "explain
this connection" triggers **one** LLM call to narrate why two items relate — opt-in,
cost-chip labelled, never on load.

---

## 4. How this strengthens RAG (concretely)

Today `search_with_context` is purely *content-match* — vector + FTS over chunk
text. A linked corpus adds a **structural recall channel**:

1. **Multi-hop retrieval**: query → top-K items (content match) → +1 graph hop along
   `shared_entity`/`explicit_ref` → enriched context set (§3.3). Answers "what else
   do I know related to this" without the user re-querying.
2. **Entity-anchored retrieval**: if the query itself contains an entity
   (`entities::extract_entities` on the query — already a cheap call), jump straight
   into `item_entities` inverted index for an exact-entity candidate set, RRF-fused
   with the vector/FTS candidates. This fixes the classic embedding miss where "张三"
   and a paraphrase don't cosine-match.
3. **Explainable citations**: because links carry `evidence`, the RAG context block
   can tell the LLM *why* item B was included ("linked to A via shared entity 张三"),
   improving answer grounding — and the cite-chain UI can show it.

No change to the embedding model, no new index type — it reuses `VectorIndex` and
`FulltextIndex`, adds one SQL join.

---

## 5. Relationship to the memory system (L1/L2/L3)

These are **complementary, not overlapping** — different axes of the same vault:

| | Memory system (`memory/`) | Internal links (`links.rs`, proposed) |
|---|---|---|
| Unit | conversations / episodic events | **vault items** (docs, notes, captures) |
| L3 `semantic.rs` does | hdbscan-clusters *episodic memories*, LLM-summarizes each topic | links *items* by entity/vector/ref, **no LLM** |
| Output | a standing prose summary per topic | a typed edge per item pair |
| Cost | L3 = third tier (LLM per topic, quota-gated) | strictly 🆓/⚡, build-time |
| Question answered | "what has the user *learned* about X over time" | "which documents *connect* to this one" |

How they reinforce each other:

- The **assembler** (`memory/assembler.rs`) picks L0/L1/L2/L3 blocks for the RAG
  prompt. Internal links slot in as an **L1-expansion**: when an L0/L1 anchor item is
  chosen, its `item_links` neighbours become candidate additional L1 blocks — the
  graph hop of §3.3 feeding the assembler.
- L3 semantic clustering and `same_cluster` links both use `hdbscan`, but on
  different corpora (memories vs item embeddings). `same_cluster` (derived link,
  §2.5) is essentially "the item-level analogue of an L3 topic". Keeping it derived
  means the two systems share the *concept* but not a table — no coupling.
- Suggested non-coupling rule: `links.rs` must not call into `memory/`, and
  `memory/` must not read `item_links` directly — they meet only at the assembler,
  which already owns tier arbitration.

---

## 6. Defect found during analysis — two divergent entity extractors

`attune-core` has **two** entity extractors and they disagree:

- `entities.rs` — `EntityKind` = Person / Money / Date / Organization (4 kinds),
  returns `Vec<Entity>` with byte offsets. Used by `entity_graph.rs` and Project
  recommendation (`entity_overlap_score`, 0.6 threshold).
- `skills/extract_entities.rs` — `Entities` struct with persons / dates / amounts /
  **locations** / organizations (5 fields), returns parsed `Amount { value, raw }`.
  Has a richer surname table and chinese-capital-number parsing.

`entity_graph.rs` is built on the *4-kind* one, so it can never produce
`location` edges — yet the v07 gap analysis explicitly wants "人/项目/**时间/地点**
关联图". Before linking ships, **consolidate to one extractor** (recommend: make
`entity_graph.rs` and the proposed `links.rs` consume `skills/extract_entities.rs`,
which has `locations` and normalized amounts; or unify `entities.rs::EntityKind` to
add `Location` and route both through it). This is a prerequisite, not optional —
otherwise the `item_entities` table is missing a whole entity class.

Note also `entity_graph.rs` itself is **dead code today** (declared `lib.rs:102`,
zero callers). This proposal is what finally makes it load-bearing — `links.rs` can
reuse `EntityGraph` / `build_from_items` verbatim for the panorama endpoint.

---

## 7. Phased implementation plan

### Phase A — foundation (prerequisite, ~0.5 day)
- Resolve the dual-extractor defect (§6): pick one extractor, add `Location` if
  using `entities.rs`. Single source of truth for entity kinds.
- Add `item_entities` + `item_links` tables to `store/mod.rs` SCHEMA_SQL
  (`CREATE TABLE IF NOT EXISTS`, auto-migrates).
- `store` CRUD: `upsert_item_entities`, `find_items_by_entity`, `replace_item_links`,
  `list_links_for_item`, `purge_item_links` — `prepare_cached` per CLAUDE.md.

### Phase B — cheap-tier link compute (~1 day)
- New `attune-core/src/links.rs`: `relink_item` (shared_entity + explicit_ref first
  — no vectors needed), `purge_item_links`. Mirror `reindex.rs` structure + tests.
- Wire `relink_item` into `reindex.rs::reindex_item` (entity + explicit_ref pass) and
  `purge_item_links` into the delete path. Out-degree cap, stop-entity filter.
- `semantic_near`: deferred pass triggered when an item's embed queue drains
  (extend embed worker / `reindex_queue`). Reuse `VectorIndex` — no new model calls.
- Unit + integration tests: link symmetry, idempotent rebuild on re-ingest, delete
  cleanup, stop-entity filtering. Golden-set regression per `docs/TESTING.md`.

### Phase C — surfacing: Reader + related route (~0.5 day)
- `GET /api/v1/items/{id}/related` in `routes/items.rs` (stored ∪ derived links).
- `Reader.tsx` "Related" panel, grouped by `relation`, i18n keys in `zh.ts`+`en.ts`
  (CLAUDE.md i18n rule — both locales, no hardcoded strings).

### Phase D — RAG link expansion (~0.5 day)
- `SearchParams.link_expansion` flag; one bounded graph hop in `search_with_context`
  after rerank; `allocate_budget` keeps linked candidates below direct hits.
- Entity-anchored candidate channel: `extract_entities(query)` → `item_entities`
  inverted-index lookup → RRF-fuse.

### Phase E — panorama graph (~0.5 day, can defer to v0.8)
- `GET /api/v1/knowledge/graph` → `entity_graph::build_from_items` over
  `item_entities`/`item_links`, emit existing `EntityGraph.to_json()`.
- `KnowledgeView.tsx` force-directed graph (replaces the `EmptyState` placeholder),
  edges colored by `relation`. Optional user-triggered LLM "explain connection"
  (third tier, cost-chip).

Total ≈ 3 day for A–D (matches the gap-analysis "Entity graph, 2 day" plus the
extractor-consolidation prerequisite and RAG wiring). Phase E is the visible payoff
and can ship a sprint later.

---

## 8. Risk / boundary notes

- **OSS boundary**: all link types here are domain-neutral (entities/vectors/refs
  are universal). Vertical relation kinds (`Corroborates` / `SameCaseNo` from
  law-pro's `evidence_chain`) stay in `attune-pro` — `links.rs` should expose a
  registration hook so a vertical plugin can contribute extra `relation` kinds, but
  ship none itself. Consistent with `docs/oss-pro-strategy.md` v2 §4.3.
- **Cost contract**: the single largest risk is link compute drifting into the LLM
  tier. The design forbids it structurally — `links.rs` has no `LlmProvider`
  parameter at all. Any LLM narration is a separate, user-triggered route.
- **Graph blow-up**: without the out-degree cap + stop-entity DF filter, a corpus
  with a few ubiquitous entities (the user's own name, "2024") produces a near-
  complete graph. The cap (top-20 edges/item) and DF filter (>30 % → drop) are
  mandatory, not tuning knobs.
- **Privacy**: `item_links` rows reference item ids and short entity strings. Item
  *content* stays in the encrypted `content` BLOB and is untouched. `item_entities`
  values are extracted entity strings — treat as low-sensitivity but keep them out
  of any outbound payload; L0 (`privacy_tier`) items' entities must not leak into a
  cloud RAG prompt via link expansion (filter linked candidates by `privacy_tier`
  the same way direct results are filtered today).
- **Incremental consistency**: links are rebuilt per item on (re)ingest; a brand-new
  item links against existing ones, but existing items don't immediately gain the
  back-link until *they* are re-touched. Mitigation: `relink_item` writes the link
  row symmetrically (both directions visible via the two indexes), so a single
  ingest *does* establish a fully queryable bidirectional link — no stale half-edge.
```
