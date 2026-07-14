# Implementation Roadmap

This document outlines what needs to be built in Arbor to achieve a production-ready authorization system.

## Current Status

### ✅ Complete (Foundation)

**arbor-types** (~600 lines):
- [x] Entity, Policy, Action, ActionSet definitions
- [x] Attributes with nested object support
- [x] Condition AST
- [x] PolicyTarget enum (Entity, EntityWithDescendants, EntityType, All)
- [x] OpCode definitions for bytecode
- [x] Type-safe StringId<T> with phantom types
- [x] Error types
- [x] IndexedEntity, IndexedPolicy, IndexedNode, IndexedEntityType types
- [x] EntityResolver trait + EvaluationContext

**arbor-graph-core** (~700 lines):
- [x] Graph storage with dense array indexing
- [x] Parent-child bidirectional relationships
- [x] Entity CRUD with circular dependency detection
- [x] Policy CRUD with validation
- [x] Basic action management
- [x] Free list for deleted nodes

**arbor-index-snapshot** (~200 lines):
- [x] Snapshot structure with flat `Vec<IndexedNode>` array
- [x] UUID ↔ u32 index mappings
- [x] Specialized bitmaps (all_principal, all_resource, conditional, forbidding, descendant_*)
- [x] `get_policies_for_principal/resource/action()` query methods
- [x] `get_principals/resources_of_type_for_policy()` listing helpers
- [x] `split_policy_map_for_authorization()` — 4-bucket policy split
- [x] `EntityResolver` impl for Snapshot

**arbor-bytecode** (~1,500 lines):
- [x] Stack-based bytecode VM (all OpCodes)
- [x] Bytecode compiler (Condition AST → bytecode)
- [x] Optimization (constant folding, dead code elimination)
- [x] EntityResolver trait for sub-entity hierarchy checks
- [x] 98 unit + 8 integration + 1 property test (500 cases)
- [x] Cedar-parity ops: StartsWith, EndsWith, StringContains, Like, IsType, InHierarchy, InNetwork

**arbor-indexer** (snapshot generation):
- [x] Transitive closure computation (ancestors + descendants via BFS)
- [x] SnapshotBuilder — single-pass with deferred write-backs
- [x] Condition compilation (AST → bytecode at index time)
- [x] Action/ActionSet expansion
- [x] Policy target mapping (UUID → index)
- [x] Entity type index population
- [x] 17 snapshot tests passing

### 🚧 Estimated Completion: ~75%

---

## V1: Minimum Viable Product

**Goal**: Production-ready authorization system with core operations and basic scalability.

**Success Criteria**:
- check() operation with <1ms p99 latency
- list_resources/principals/actions with <10ms p99 latency
- Supports 100K entities, 10K policies
- Horizontally scalable authorizers
- Basic observability (logs, metrics, tracing)

---

### Phase 1: Core Authorization Engine ✅ Complete

#### Step 1: Snapshot Serialization (arbor-index-snapshot)

**Priority**: 🔴 Critical
**Status**: ✅ Complete

The Snapshot struct currently lives only in memory. We need to serialize it so the indexer can produce a file/blob and the authorizer can load it.

**Tasks**:
1. Add `serde` derives to all types that appear in the snapshot
   - `Snapshot`, `IndexedNode`, `IndexedEntity`, `IndexedPolicy`, `IndexedEntityType`
   - `CompiledCondition`, `OpCode`, `VariableRef`, `Attributes`, `AttributeValue`
   - `IndexedPolicyTarget`, `EntityTypeId`, `RoaringBitmap` (via `roaring/serde`)
   - Note: RapidHashMap will need custom serde or conversion to/from HashMap
2. Add `PackagedSnapshot` wrapper struct:
   ```rust
   pub struct PackagedSnapshot {
       pub version: u64,
       pub checksum: [u8; 32],  // blake3 or xxhash
       pub created_at_ms: i64,
       pub metadata: SnapshotMetadata,
       pub data: Snapshot,
   }
   pub struct SnapshotMetadata {
       pub entity_count: u32,
       pub policy_count: u32,
       pub action_count: u32,
       pub generation_duration_ms: u64,
   }
   ```
3. Serialize with `bincode` (fast, compact, Rust-native)
4. Compress with `lz4` or `zstd` (fast decompression is key for authorizers)
5. Compute checksum over serialized bytes (blake3 — fast, cryptographic)
6. `PackagedSnapshot::serialize() -> Vec<u8>` and `PackagedSnapshot::deserialize(&[u8]) -> Result<Self>`

**Key decisions**:
- Use `bincode` for V1 (fast, simple). Protobuf migration possible in V2 if cross-language snapshot sharing is needed.
- Use `blake3` for checksums (faster than SHA-256, cryptographic quality).
- Compression: `lz4` for speed or `zstd` for ratio — pick one.
- RapidHashMap serde: serialize as sorted Vec<(K,V)> pairs, reconstruct on deserialize.

**Crates to add**: `serde`, `bincode`, `blake3` (or `xxhash-rust`), `lz4_flex` (or `zstd`)

**Files to modify**:
- `crates/arbor-types/src/*.rs` — add serde derives
- `crates/arbor-index-snapshot/src/lib.rs` — add PackagedSnapshot, serialize/deserialize
- `crates/arbor-index-snapshot/Cargo.toml` — add deps

**Estimated Lines**: ~300

#### Step 2: Authorization Operations

**Priority**: 🔴 Critical
**Status**: ✅ Complete

The Snapshot already has the query helpers (`get_policies_for_*`, `split_policy_map_for_authorization`). Now implement the actual authorization logic.

**Tasks**:
1. Implement `check()` in a new `arbor-authorizer` crate (library, not service)
   - Get applicable policies (principal ∩ resource ∩ action bitmap intersection)
   - Split into 4 categories via existing `split_policy_map_for_authorization()`
   - Evaluate with short-circuiting (uncond forbid → cond forbid → uncond permit → cond permit)
   - Return decision + optional reason (for explain mode)
   - Fail closed: Unknown/Invalid on forbid → deny; on permit → skip

2. Implement `list_resources()`
   - Phase 1: Bitmap ops for unconditional policies
   - Phase 2: Conditional evaluation for residuals
   - Pagination support

3. Implement `list_principals()` — symmetric to list_resources()

4. Implement `list_actions()`
   - Simpler: iterate applicable policies, collect actions, subtract forbidden

5. Define request/response types
   ```rust
   pub struct CheckRequest { principal: u32, action: u32, resource: u32, context: Attributes, explain: bool }
   pub enum Decision { Permit, Deny }
   pub struct CheckResponse { decision: Decision, reason: Option<Reason> }
   ```

**Important**: Authorization functions operate on **u32 indices**, not UUIDs. UUID→index resolution happens at the API boundary (service layer), not in the core engine. This keeps the hot path allocation-free.

**Where to put this code**:
- Option A: New `crates/arbor-authorizer/` crate (authorization logic library)
- Option B: Inside `services/arbor-authorizer/src/` (couples logic to service)
- **Recommended**: Option A — keeps authorization logic reusable and testable without service dependencies

**Files to create**:
- `crates/arbor-authorizer/src/lib.rs`
- `crates/arbor-authorizer/src/check.rs`
- `crates/arbor-authorizer/src/list.rs`
- `crates/arbor-authorizer/src/types.rs` (request/response types)

**Estimated Lines**: ~800

#### Step 3: End-to-End Integration Tests

**Priority**: 🔴 Critical
**Status**: 🚧 Not started ← YOU ARE HERE

Test the full pipeline: Graph → SnapshotBuilder → Snapshot → check()/list()

**Tasks**:
1. Build a realistic test graph (users, groups, files, folders, policies)
2. Generate snapshot via SnapshotBuilder
3. Serialize → deserialize round-trip
4. Run authorization queries and verify correctness
5. Test forbid precedence, hierarchy, conditions, edge cases

**Files to create**:
- `tests/integration/authorization_e2e.rs`

**Estimated Lines**: ~500

---

### Phase 2: Services and Distribution

#### Step 4: Indexer Service

**Priority**: 🔴 Critical
**Status**: ✅ Complete

Indexer loads a graph, builds a `PackagedSnapshot`, and writes it to `ARBOR_SNAPSHOT_PATH`. Currently uses `example_graph::build()` as its data source — replaced by connectors in Step 4b.

#### Step 4b: Connectors (arbor-connectors)

**Priority**: 🔴 Critical
**Status**: 🚧 Not started

Replace the hardcoded `example_graph::build()` with a real data ingestion layer driven by YAML config.

**Design**: Two config files:
- `config/connectors.yaml` — named connection definitions (credentials injected via `ARBOR__CONNECTORS__<NAME>__PASSWORD`)
- `config/entity_types.yaml` — SQL queries per entity type + policy queries, each referencing a connector by name

See [connectors.md](./connectors.md) for full configuration reference.

**Tasks**:
1. Add `ConnectorsConfig`, `EntityTypesConfig`, and config loading to `arbor-connectors`
2. Define `Connector` trait: `fn load(&self) -> ArborResult<Graph>`
3. Implement `ExampleConnector` (wraps `example_graph::build()`)
4. Implement `PostgresConnector` using `sqlx` — runs entity + policy queries, assembles `Graph`
5. Wire into `services/arbor-indexer/src/main.rs` (replace `example_graph::build()`)

**Files to create/modify**:
- `crates/arbor-connectors/src/lib.rs` — config types + `Connector` trait
- `crates/arbor-connectors/src/postgres.rs` — `PostgresConnector`
- `crates/arbor-connectors/src/example.rs` — `ExampleConnector`
- `crates/arbor-connectors/Cargo.toml` — add `sqlx`, `serde`, `config`, `tokio`
- `services/arbor-indexer/src/main.rs` — load connector from config

**Crates to add**: `sqlx = { version = "0.8", features = ["postgres", "uuid", "runtime-tokio"] }`

**Estimated Lines**: ~400

#### Step 4c: CSV Connector

**Priority**: 🟡 High
**Status**: 🚧 Not started

Simple file-based connector for bootstrapping and testing without a live database.

**Tasks**:
1. Implement `CsvConnector` — reads `entities.csv` and `policies.csv`
2. Add `csv` connector type to `ConnectorConfig`

**CSV column contracts** mirror the SQL query contracts:
- `entities.csv`: `id, name, type_name, parent_ids` (semicolon-separated UUIDs for `parent_ids`)
- `policies.csv`: `id, name, policy_type, principal_id, resource_id, actions` (semicolon-separated UUIDs for `actions`)

**Config**:
```yaml
connectors:
  flat_files:
    type: csv
    entities_file: /data/entities.csv
    policies_file: /data/policies.csv
```

**Crates to add**: `csv = "1"`

**Estimated Lines**: ~150

#### Step 5: Authorizer Service (Dual Transport)

**Priority**: 🔴 Critical
**Status**: ✅ Complete

**V1 approach**: Authorizer loads a snapshot file on startup from `ARBOR_SNAPSHOT_PATH` env var. No file watching — restart to reload.

**Tasks**:
1. Service skeleton
2. Load snapshot file on startup, deserialize, verify checksum
3. Unix Domain Socket server (length-prefixed protobuf)
5. gRPC server (same protobuf messages)
6. Route requests to authorization engine (crates/arbor-authorizer)
7. UUID→index resolution at API boundary
8. Health checks (readiness = has snapshot, liveness = process alive)
9. Configuration

**Files to create**:
- `services/arbor-authorizer/src/service.rs`
- `services/arbor-authorizer/src/snapshot_manager.rs`
- `services/arbor-authorizer/src/transports/unix_socket.rs`
- `services/arbor-authorizer/src/transports/grpc.rs`
- `services/arbor-authorizer/src/config.rs`
- `crates/arbor-proto-internal/proto/arbor/v1/arbor.proto`

**Estimated Lines**: ~1,400

#### Step 6: Integration & Load Testing

**Priority**: 🔴 Critical

- End-to-end indexer → authorizer flow
- Load testing with k6/Locust
- Correctness test suite

---

## V1 Deliverables

By end of V1, Arbor will have:

- ✅ Complete bytecode VM with all OpCodes
- ✅ Snapshot builder with transitive closures
- ✅ Serializable snapshots with checksums
- ✅ All 4 authorization operations (check, list×3)
- ✅ Indexer service (writes snapshot file)
- ✅ Authorizer service with dual transport (UDS + gRPC)
- 🚧 E2E integration tests (Graph → Snapshot → check → verify)
- 🚧 PostgreSQL connector with YAML config
- 🚧 Integration and load tests

**Note**: Delta support, client libraries, and observability deferred to post-V1 (saves 2-3 weeks on core engine).

---

## V2: Production Hardening

**Goal**: Enterprise-ready with HA, persistence, and advanced features.

### Distribution and HA
- Broker-based snapshot notifications (NATS, Kafka, etc.) — replace file-based transfer
- External snapshot storage (S3/blob)
- Indexer HA (leader election)
- Database connectors (PostgreSQL, MySQL, CDC)
- Incremental indexing

### Advanced Features
- Temporal policies (time-based conditions)
- Policy testing framework
- Advanced caching (Redis)
- Delta snapshots (incremental updates)

### Enterprise Features
- Audit logging
- Policy versioning
- Multi-tenancy
- Admin UI

---

## Post-V1: Client Libraries and Observability

After V1 core engine ships, add:

### Client Libraries
- Rust client (arbor-client)
- Node.js client (@arbor/client)
- Python client (arbor-client)
- All with auto-detecting transport (UDS vs gRPC)
- **Estimated Lines**: ~2,000 (across languages)

### Observability
- OpenTelemetry tracing (spans for check/list operations)
- Prometheus metrics (latency histograms, permit/deny counters)
- Structured logging via `tracing`
- Example Grafana dashboards

---

## V3: Scale and Optimization (Future)

- JIT compilation for hot conditions
- SIMD batch evaluation
- Query plan optimization
- Sharded indexers
- Cross-region replication

---

## Dependencies

### V1
```toml
# Core
tokio = { version = "1", features = ["full"] }
uuid = { version = "1", features = ["v4", "serde"] }
serde = { version = "1", features = ["derive"] }

# Performance
roaring = { version = "0.11", features = ["serde"] }
rapidhash = "4"
smallvec = "1"

# Serialization
bincode = "1"
blake3 = "1"
lz4_flex = "0.11"

# HTTP/gRPC
tonic = "0.12"
prost = "0.13"
axum = "0.8"

# Testing
proptest = "1"
criterion = "0.5"
```

### Post-V1 (Observability & Client Libraries)
```toml
# Observability
tracing = "0.1"
tracing-subscriber = "0.3"
opentelemetry = "0.26"
opentelemetry-jaeger = "0.26"
metrics = "0.21"
metrics-exporter-prometheus = "0.14"
```

### V1 (connectors)
```toml
sqlx = { version = "0.8", features = ["postgres", "uuid", "runtime-tokio"] }
```

### V2+
```toml
aws-sdk-s3 = "1"
etcd-client = "0.14"
```

---

## Immediate Next Steps

1. **Now**: PostgreSQL connector (`arbor-connectors`) with YAML config (Step 4b)
2. **Then**: CSV connector
3. **Then**: E2E integration tests (Graph → Snapshot → serialize → deserialize → check → verify)
4. **After that**: Load testing (indexer → authorizer flow with k6/Locust)

---

## Related Documentation

- [Architecture](./architecture.md) - System design
- [Authorization Flow](./authorization-flow.md) - How operations work
- [Bytecode VM](./bytecode-vm.md) - VM implementation details
- [Snapshot Format](./snapshot-format.md) - Data structures
- [Data Model](./data-model.md) - Core types
- [Pub/Sub Protocol](./pub-sub-protocol.md) - Broker protocol
