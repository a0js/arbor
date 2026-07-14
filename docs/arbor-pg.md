# arbor-pg: PostgreSQL Extension (Add-On Feature)

> **Status**: Potential future feature. Not part of V1 or V2 core. Requires the Rust client library (`arbor-client`) to be complete first.

`arbor-pg` is a native PostgreSQL extension that exposes Arbor authorization as SQL-callable functions. Built with [pgrx](https://github.com/pgcentralfoundation/pgrx), it compiles Rust directly into a PostgreSQL shared library (`.so`), giving you authorization primitives usable in `WHERE` clauses, Row Level Security policies, and `pgvector` ANN searches — with no application-level changes required.

---

## Why a PostgreSQL Extension?

Most authorization integrations require application code to resolve permissions before every query. This has two problems:

1. **Enforcement gaps**: Application code can be bypassed; the database cannot.
2. **Coupling**: Every query path must be updated when authorization requirements change.

A native PostgreSQL extension solves both: authorization is enforced at the database layer and is transparent to the application. It also integrates naturally with `pgvector` for AI/RAG workloads where authorized document filtering must happen before or during vector search.

---

## Architecture

```
┌─────────────────────────────┐
│     PostgreSQL Process      │
│                             │
│  ┌───────────────────────┐  │
│  │  arbor-pg extension   │  │
│  │  (pgrx, Rust)         │  │
│  │                       │  │
│  │  arbor_check()        │  │  Unix socket
│  │  arbor_permitted_ids()│ ─┼──────────────────► Arbor Authorizer
│  │  arbor_batch_check()  │  │                    (sidecar)
│  └───────────────────────┘  │
│                             │
└─────────────────────────────┘
```

The extension embeds `arbor-client` and maintains a persistent connection pool to the Arbor sidecar via Unix Domain Socket. Connection overhead is paid once per PostgreSQL backend process, not per query.

---

## SQL Functions

### `arbor_check(principal_id uuid, action text, resource_id uuid) → boolean`

Single authorization check. Use in `WHERE` clauses or as a guard for small result sets only. **Do not use in RLS policies evaluated per-row** — see the RLS section below.

```sql
SELECT * FROM documents
WHERE id = 'some-doc-id'
  AND arbor_check(current_setting('app.user_id')::uuid, 'read', id);
```

### `arbor_permitted_ids(principal_id uuid, action text, entity_type text) → SETOF uuid`

Returns the full set of resource UUIDs the principal is permitted to perform `action` on, for the given entity type. Issues a single `list_resources()` call to the Arbor sidecar. This is the primary function for pre-filtering queries.

```sql
SELECT content, embedding <-> $1 AS distance
FROM documents
WHERE id IN (
  SELECT * FROM arbor_permitted_ids(
    current_setting('app.user_id')::uuid, 'read', 'document'
  )
)
ORDER BY distance
LIMIT 10;
```

### `arbor_batch_check(principal_id uuid, action text, resource_ids uuid[]) → uuid[]`

Takes an array of candidate resource IDs, issues a single `batch_check()` call to Arbor, and returns the subset that is permitted. Designed for retrieve-then-check patterns (e.g., post-vector-search authorization).

```sql
-- After ANN search, filter results through Arbor in one round-trip
SELECT content, distance
FROM (
  SELECT id, content, embedding <-> $1 AS distance
  FROM documents
  ORDER BY distance
  LIMIT 100
) candidates
WHERE id = ANY(
  arbor_batch_check(
    current_setting('app.user_id')::uuid,
    'read',
    array_agg(id) OVER ()
  )
)
ORDER BY distance
LIMIT 10;
```

---

## Usage Patterns

### Pattern 1: Session-Local Materialization (Recommended for most queries)

Materialize the authorized ID set once at the start of a request, reuse it across all queries in that session/transaction:

```sql
-- At request start (once per request, ~200–600μs)
CREATE TEMP TABLE permitted_docs ON COMMIT DROP AS
  SELECT id FROM arbor_permitted_ids(
    current_setting('app.user_id')::uuid, 'read', 'document'
  );

CREATE INDEX ON permitted_docs(id);  -- if queried multiple times

-- All subsequent queries in this request (fast, no Arbor calls)
SELECT * FROM documents
JOIN permitted_docs USING (id)
WHERE ...;

SELECT content, embedding <-> $1 AS distance
FROM documents
JOIN permitted_docs USING (id)
ORDER BY distance
LIMIT 10;
```

### Pattern 2: Retrieve-Then-Batch-Check (Best for large datasets with pgvector)

Let the vector search find semantic matches first, then authorize only the top candidates. Avoids passing large ID sets to the vector index:

```sql
WITH candidates AS (
  SELECT id, content, embedding <-> $1 AS distance
  FROM documents
  ORDER BY distance
  LIMIT 100         -- over-retrieve to account for authorization filtering
),
permitted AS (
  SELECT unnest(
    arbor_batch_check(
      current_setting('app.user_id')::uuid,
      'read',
      (SELECT array_agg(id) FROM candidates)
    )
  ) AS id
)
SELECT c.content, c.distance
FROM candidates c
JOIN permitted p ON c.id = p.id
ORDER BY distance
LIMIT 10;
```

**When to prefer this over Pattern 1**: When the authorized set is large (>10K IDs) and the query is vector-search-driven. The ANN index is more effective when it can search without a large pre-filter, and `batch_check()` on 100 candidates is a single cheap Arbor call.

### Pattern 3: Row Level Security (Careful — see warning)

RLS enforces authorization at the database level — impossible for application code to bypass. Use `arbor_permitted_ids()` in the policy, not `arbor_check()`, to avoid per-row Arbor calls:

```sql
-- DO NOT do this — arbor_check() is called once per row scanned
CREATE POLICY arbor_read_bad ON documents
  USING (arbor_check(current_setting('app.user_id')::uuid, 'read', id));

-- DO this — arbor_permitted_ids() is called once per query, not per row
CREATE POLICY arbor_read ON documents
  USING (
    id IN (
      SELECT * FROM arbor_permitted_ids(
        current_setting('app.user_id')::uuid, 'read', 'document'
      )
    )
  );

ALTER TABLE documents ENABLE ROW LEVEL SECURITY;
```

PostgreSQL's query planner will evaluate the subquery once per statement and use it as a filter, not re-evaluate it per row, as long as the function is marked `STABLE` (which `arbor_permitted_ids` will be).

> **Note**: RLS is a strong enforcement primitive but adds non-trivial overhead for tables with millions of rows because the authorization filter is applied to every candidate row before the query's own `WHERE` clause can prune. Profile before deploying RLS on hot tables. Pattern 1 or Pattern 2 are preferable where enforcement can be trusted to the application layer.

---

## Performance Characteristics

All numbers assume the Arbor sidecar is co-located (Unix socket, no network hop).

| Operation | Single call | Per-row RLS | Notes |
|---|---|---|---|
| `arbor_check()` | ~50–80μs | O(rows) — avoid | Use only for single-row lookups |
| `arbor_permitted_ids()` | ~200–600μs | N/A | One call per query |
| `arbor_batch_check(100)` | ~200–400μs | N/A | One round-trip for 100 candidates |
| Session-local temp table | ~200–600μs setup, ~0 thereafter | N/A | Amortizes across multi-query requests |

### Effect on total query latency

```
Typical pgvector ANN search:         ~2–10ms
+ arbor_permitted_ids() (Pattern 1):  ~200–600μs overhead  (~5–20% of query time)
+ arbor_batch_check() (Pattern 2):    ~200–400μs overhead  (~3–15% of query time)
```

### Scaling limit for `arbor_permitted_ids()`

The authorized ID set is transferred from Arbor to PostgreSQL as a UUID array. Performance degrades as the set grows:

| Authorized IDs | Transfer size | `WHERE id IN (...)` behavior | Recommendation |
|---|---|---|---|
| < 1,000 | < 36KB | Index scan, fast | Any pattern works |
| 1K–10K | 36–360KB | Bitmap heap scan | Pattern 1 or 2 |
| 10K–100K | 360KB–3.6MB | Approaches sequential scan | Switch to metadata filtering |
| 100K+ | > 3.6MB | Unusable | Metadata filtering required |

For large authorized sets, push authorization structure into document metadata at index time (e.g., `department`, `sensitivity_level`, `owner_group`) and filter by those categorical values instead of explicit ID lists. Arbor's `EntityWithDescendants` and `EntityType` policy targets map naturally to categorical metadata: access granted to a group can be expressed as `WHERE department = 'engineering'` rather than listing every document ID.

---

## RAG / AI Workloads

`arbor-pg` is particularly valuable for Retrieval-Augmented Generation pipelines using `pgvector`. It ensures the LLM only receives document chunks the requesting user is authorized to read.

**Recommended RAG authorization flow**:

```sql
-- 1. Light categorical pre-filter (fast, uses metadata index)
-- 2. ANN search over filtered candidates
-- 3. batch_check() as safety net on top results
-- 4. Return only authorized chunks to the LLM

WITH categorically_filtered AS (
  SELECT id, content, embedding
  FROM document_chunks
  WHERE department = current_setting('app.department')   -- categorical pre-filter
    AND sensitivity <= current_setting('app.clearance')  -- from Arbor's entity metadata
),
top_candidates AS (
  SELECT id, content, embedding <-> $1 AS distance
  FROM categorically_filtered
  ORDER BY distance
  LIMIT 100
),
authorized AS (
  SELECT unnest(
    arbor_batch_check(
      current_setting('app.user_id')::uuid,
      'read',
      (SELECT array_agg(id) FROM top_candidates)
    )
  ) AS id
)
SELECT tc.content
FROM top_candidates tc
JOIN authorized a ON tc.id = a.id
ORDER BY tc.distance
LIMIT 10;
```

This pattern:
- Uses metadata pre-filtering to shrink the ANN search space without large ID lists
- Uses `batch_check()` as a fine-grained safety net (one round-trip for 100 candidates)
- Never passes unauthorized content to the LLM

---

## Implementation Plan

**Prerequisites**:
- `arbor-client` Rust library (connection pooling, Unix socket transport, `batch_check()` API)
- `batch_check()` operation in the Arbor authorizer (planned post-V1)

**Tasks**:

1. Create `crates/arbor-pg/` as a new crate with pgrx dependency
2. Implement connection management: one pooled `ArborClient` per PostgreSQL backend process
3. Implement `arbor_check()` — thin wrapper over `client.check()`
4. Implement `arbor_permitted_ids()` — wraps `client.list_resources()`, returns `SetOfIterator<Uuid>`
5. Implement `arbor_batch_check()` — wraps `client.batch_check()`, accepts `Vec<Uuid>`, returns `Vec<Uuid>`
6. Mark `arbor_permitted_ids()` and `arbor_batch_check()` as `STABLE` (same input → same output within a statement)
7. Error handling: connection failures and Arbor errors must surface as PostgreSQL errors, not panics
8. Integration tests using `pgrx`'s test framework (spins up a real PostgreSQL instance)
9. Installation script and `arbor-pg.control` extension manifest

**Key pgrx considerations**:
- pgrx runs Rust code inside the PostgreSQL process — panics abort the backend. All Arbor client calls must use `Result` and convert errors to PostgreSQL errors via `pgrx::error!()`.
- The `ArborClient` connection pool must be stored in PostgreSQL's `static` process-local memory (pgrx `pg_module_magic!` / `_PG_init` hook), not on the stack.
- `STABLE` annotation tells the planner the function can be called once per statement. Do not mark functions as `IMMUTABLE` — Arbor authorization state changes when snapshots reload.

**Estimated lines**: ~600

---

## Dependencies

```toml
[dependencies]
pgrx = "0.12"
arbor-client = { path = "../../crates/arbor-client" }  # must exist first
uuid = { version = "1", features = ["v4"] }
tokio = { version = "1", features = ["rt"] }           # for async client calls within pgrx

[dev-dependencies]
pgrx-tests = "0.12"
```

---

## Related Documentation

- [Client Libraries](./client-libraries.md) — `arbor-client` (prerequisite for this extension)
- [Authorization Flow](./authorization-flow.md) — how `check()` and `list_resources()` work internally
- [Implementation Roadmap](./implementation-roadmap.md) — where this fits in the delivery plan
