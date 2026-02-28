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

**arbor-graph-core** (~700 lines):
- [x] Graph storage with dense array indexing
- [x] Parent-child bidirectional relationships
- [x] Entity CRUD with circular dependency detection
- [x] Policy CRUD with validation
- [x] Basic action management
- [x] Free list for deleted nodes

**arbor-index-snapshot** (~400 lines):
- [x] IndexSnapshot structure
- [x] IndexedEntity with ancestor/descendant bitmaps
- [x] IndexedPolicy with compiled conditions
- [x] Query helper methods (get_policies_for_*)
- [x] Specialized bitmaps for optimization

**arbor-bytecode**:
- [x] OpCode enum defined in arbor-types
- [x] Bytecode VM implementation (complete, 55 tests passing)

**Services** (stubs):
- [ ] arbor-indexer (basic skeleton)
- [ ] arbor-authorizer (basic skeleton)

### 🚧 Estimated Completion: 32%

## V1: Minimum Viable Product

**Goal**: Production-ready authorization system with core operations and basic scalability.

**Timeline**: 8-10 weeks (assuming 1 engineer full-time)

**Success Criteria**:
- check() operation with <1ms p99 latency
- list_resources/principals/actions with <10ms p99 latency
- Supports 100K entities, 10K policies
- Horizontally scalable authorizers
- Basic observability (logs, metrics, tracing)

---

### Phase 1: Core Authorization Engine (4-6 weeks)

#### Week 1-2: Bytecode VM ✅

**Priority**: 🔴 Critical

**Tasks**:
1. Implement `BytecodeVM` in `arbor-bytecode/src/bytecode_vm.rs`
   - [x] Stack-based interpreter
   - [x] Instruction execution for all OpCodes
   - [x] EvaluationContext with attribute resolution
   - [x] Variable resolution from principal/resource/context
   - [x] Missing attribute semantics (Missing sentinel → false at comparisons)
   - [x] EntityResolver trait for sub-entity hierarchy checks

2. Implement `BytecodeCompiler` in `arbor-bytecode/src/compiler.rs`
   - [x] Condition AST → bytecode compilation
   - [x] Optimization (constant folding, dead code elimination)
   - [x] Jump instruction patching for control flow

3. Tests
   - [x] Unit tests for each OpCode (55 tests)
   - [x] Integration tests (compile + evaluate end-to-end)
   - [x] Property-based tests (bytecode ≡ AST evaluation)

**Cedar-parity ops added**: `StartsWith`, `EndsWith`, `StringContains`, `Like` (glob),
`IsType`, `InHierarchy`, `InHierarchyVar`, `ContainsInHierarchy`

**Files Created**:
- `crates/arbor-bytecode/src/bytecode_vm.rs`
- `crates/arbor-types/src/evaluation.rs` (EntityResolver trait, EvaluationContext)

#### Week 2-3: Snapshot Builder

**Priority**: 🔴 Critical

**Tasks**:
1. Implement transitive closure computation
   - [x] `compute_ancestors()` - BFS/DFS to find all ancestors
   - [x] `compute_descendants()` - BFS/DFS to find all descendants
   - [x] Cycle detection validation

2. Implement `SnapshotBuilder` in `services/arbor-indexer/src/snapshot_builder.rs`
   - [ ] Build UUID ↔ index mappings
   - [ ] Create IndexedEntity from Entity + closures
   - [ ] Compile conditions to bytecode
   - [ ] Create IndexedPolicy from Policy + compiled conditions
   - [ ] Build entity type indexes
   - [ ] Build action → policy indexes
   - [ ] Build specialized bitmaps
   - [ ] Compute checksums

3. Implement batching strategy (TBD: time-based, count-based, or debounced)
   - [ ] Define batch trigger conditions
   - [ ] Batch buffer for pending changes
   - [ ] Snapshot generation throttling

4. Tests
   - [x] Closure computation correctness
   - [ ] Snapshot generation from graph
   - [ ] Checksum verification
   - [ ] Batching logic tests

**Files to Create**:
- `services/arbor-indexer/src/snapshot_builder.rs`
- `services/arbor-indexer/src/closures.rs`
- `services/arbor-indexer/tests/snapshot_tests.rs`

**Estimated Lines**: ~1,200

#### Week 3-4: Authorization Operations

**Priority**: 🔴 Critical

**Tasks**:
1. Implement `check()` in `arbor-authorizer/src/check.rs`
   - [ ] Get applicable policies (principal ∩ resource ∩ action)
   - [ ] Split into 4 categories (uncond forbid, cond forbid, uncond permit, cond permit)
   - [ ] Evaluate with short-circuiting
   - [ ] Return decision + optional reason
   - [ ] Error handling

2. Implement `list_resources()` in `arbor-authorizer/src/list.rs`
   - [ ] Phase 1: Bitmap operations for unconditional policies
   - [ ] Phase 2: Conditional evaluation for residuals
   - [ ] Attribute shape caching optimization
   - [ ] Pagination support

3. Implement `list_principals()` in `arbor-authorizer/src/list.rs`
   - [ ] Symmetric to list_resources()
   - [ ] Same two-phase strategy

4. Implement `list_actions()` in `arbor-authorizer/src/list.rs`
   - [ ] Simpler: just check all actions
   - [ ] Expand action sets

5. Tests
   - [ ] check() unit tests (various scenarios)
   - [ ] list_resources() correctness
   - [ ] list_principals() correctness
   - [ ] list_actions() correctness
   - [ ] Forbid precedence validation
   - [ ] Short-circuit verification

**Files to Create**:
- `services/arbor-authorizer/src/check.rs`
- `services/arbor-authorizer/src/list.rs`
- `services/arbor-authorizer/src/evaluator.rs`
- `services/arbor-authorizer/tests/authorization_tests.rs`

**Estimated Lines**: ~1,200

#### Week 4-5: Helper Functions

**Priority**: 🟡 Important

**Tasks**:
1. Attribute path resolution
   - [ ] `Attributes::resolve_path()` method
   - [ ] Nested object traversal
   - [ ] Error handling for missing attributes

2. Action/ActionSet management in graph
   - [ ] Complete `add_action()` in arbor-graph-core
   - [ ] Implement `upsert_action_set()`
   - [ ] Implement `remove_action_set()`
   - [ ] Action → EntityType association

3. Policy split helper
   - [ ] `split_policy_map_for_authorization()` in arbor-index-snapshot
   - [ ] Categorize into 4 buckets

4. Tests
   - [ ] Attribute path resolution edge cases
   - [ ] Action/ActionSet CRUD
   - [ ] Policy splitting correctness

**Files to Modify**:
- `crates/arbor-types/src/attributes.rs`
- `crates/arbor-graph-core/src/mutations.rs`
- `crates/arbor-index-snapshot/src/lib.rs`

**Estimated Lines**: ~500

---

### Phase 2: Services and Distribution (2-3 weeks)

#### Week 5-6: Indexer Service

**Priority**: 🔴 Critical

**Tasks**:
1. Implement `arbor-indexer` service
   - [ ] Service skeleton with async runtime (tokio)
   - [ ] Graph initialization
   - [ ] Snapshot generation with batching
   - [ ] Pub/sub publisher (NATS for V1, broker-agnostic interface)
   - [ ] HTTP/gRPC server for full snapshot requests
   - [ ] HTTP server for health checks

2. Implement connector interface
   - [ ] Trait for data source connectors
   - [ ] Mock connector for testing
   - [ ] CSV connector (simple V1)

3. Configuration
   - [ ] Config file format (TOML/YAML)
   - [ ] Environment variable overrides
   - [ ] CLI arguments

4. Tests
   - [ ] Snapshot generation end-to-end
   - [ ] Snapshot notification publication
   - [ ] Full snapshot serving

**Files to Create**:
- `services/arbor-indexer/src/main.rs`
- `services/arbor-indexer/src/service.rs`
- `services/arbor-indexer/src/connectors/mod.rs`
- `services/arbor-indexer/src/connectors/csv.rs`
- `services/arbor-indexer/src/config.rs`

**Estimated Lines**: ~800

#### Week 6-7: Authorizer Service (Dual Transport)

**Priority**: 🔴 Critical

**Tasks**:
1. Implement `arbor-authorizer` service
   - [ ] Service skeleton with async runtime
   - [ ] Snapshot initialization (fetch from indexer)
   - [ ] Pub/sub subscriber (NATS for V1)
   - [ ] Snapshot update with checksum verification
   - [ ] Atomic snapshot swapping
   - [ ] **Dual transport support**: Unix Socket + gRPC
   - [ ] Health checks (readiness, liveness)

2. Unix Domain Socket server
   - [ ] Length-prefixed Protobuf protocol
   - [ ] Connection pooling
   - [ ] Disable Nagle's algorithm (setNoDelay)
   - [ ] Abstract socket support with fallback (Linux optimization)
   - [ ] Error handling and reconnection

3. gRPC server
   - [ ] Standard gRPC/HTTP2 implementation
   - [ ] Same Protobuf messages as Unix socket
   - [ ] TLS support (optional)

4. API definitions (shared between transports)
   - [ ] Protocol buffer definitions in arbor-proto-internal
   - [ ] Check, ListResources, ListPrincipals, ListActions
   - [ ] Request/Response types
   - [ ] Error mapping

5. Configuration
   - [ ] Mode selection (sidecar, centralized, both)
   - [ ] Unix socket path configuration
   - [ ] gRPC address/port configuration
   - [ ] Indexer connection settings
   - [ ] Pub/sub settings

6. Tests
   - [ ] Snapshot updates
   - [ ] Unix socket API tests
   - [ ] gRPC API tests
   - [ ] Dual mode tests
   - [ ] Hot swapping validation

**Files to Create**:
- `services/arbor-authorizer/src/main.rs`
- `services/arbor-authorizer/src/service.rs`
- `services/arbor-authorizer/src/snapshot_manager.rs`
- `services/arbor-authorizer/src/transports/unix_socket.rs`
- `services/arbor-authorizer/src/transports/grpc.rs`
- `services/arbor-authorizer/src/config.rs`
- `crates/arbor-proto-internal/proto/arbor/v1/arbor.proto`
- `crates/arbor-proto-internal/build.rs` (protoc build script)

**Estimated Lines**: ~1,400

#### Week 7-8: Client Libraries

**Priority**: 🔴 Critical

**Tasks**:
1. Protobuf code generation
   - [ ] Set up protoc for all target languages
   - [ ] Generate Rust client stubs (prost + tonic)
   - [ ] Generate Node.js client stubs (grpc-js)
   - [ ] Generate Python client stubs (grpcio)

2. Rust client library (`arbor-client` crate)
   - [ ] Transport trait abstraction
   - [ ] UnixSocketTransport implementation
   - [ ] GrpcTransport implementation
   - [ ] Auto-detection logic
   - [ ] Connection pooling/reuse
   - [ ] Error handling
   - [ ] Unit tests

3. Node.js client library (`@arbor/client` npm package)
   - [ ] Transport abstraction class
   - [ ] UnixSocketTransport implementation
   - [ ] GrpcTransport implementation
   - [ ] Auto-detection logic
   - [ ] Connection pooling/reuse
   - [ ] TypeScript definitions
   - [ ] Unit tests

4. Python client library (`arbor-client` PyPI package)
   - [ ] Transport abstraction
   - [ ] UnixSocketTransport implementation
   - [ ] GrpcTransport implementation
   - [ ] Auto-detection logic
   - [ ] Type hints
   - [ ] Unit tests

5. Documentation
   - [ ] API documentation for each language
   - [ ] Usage examples
   - [ ] Migration guides (from Cedar, OpenFGA)
   - [ ] Performance benchmarks

6. Publishing
   - [ ] Publish to crates.io (Rust)
   - [ ] Publish to npm (Node.js)
   - [ ] Publish to PyPI (Python)

**Files to Create**:
- `clients/rust/arbor-client/src/lib.rs`
- `clients/rust/arbor-client/src/transport.rs`
- `clients/rust/arbor-client/src/transports/unix_socket.rs`
- `clients/rust/arbor-client/src/transports/grpc.rs`
- `clients/nodejs/@arbor/client/src/index.js`
- `clients/nodejs/@arbor/client/src/transport.js`
- `clients/nodejs/@arbor/client/src/transports/unix-socket.js`
- `clients/nodejs/@arbor/client/src/transports/grpc.js`
- `clients/python/arbor_client/__init__.py`
- `clients/python/arbor_client/transport.py`
- `clients/python/arbor_client/transports/unix_socket.py`
- `clients/python/arbor_client/transports/grpc.py`

**Estimated Lines**: ~2,000 (across all languages)

#### Week 8: Broker Connectors

**Priority**: 🔴 Critical

**Note**: Broker is for **indexer → authorizer** communication (snapshot availability notifications), not client → authorizer.

**Tasks**:
1. Implement broker abstraction in `arbor-connectors`
   - [ ] MessageBroker trait (connect, publish, subscribe, close)
   - [ ] MessageStream trait (next message)
   - [ ] Message struct (payload, timestamp)
   - [ ] NATS implementation (V1 reference)
   - [ ] In-memory implementation (for testing)

2. Message format
   - [ ] SnapshotAvailableMessage struct
   - [ ] MessagePack serialization (compact, fast)
   - [ ] Checksum verification

3. Error handling
   - [ ] Retry logic with exponential backoff
   - [ ] Connection recovery
   - [ ] Checksum verification on receive

**Files to Create**:
- `crates/arbor-connectors/src/broker.rs`
- `crates/arbor-connectors/src/nats.rs`
- `crates/arbor-connectors/src/memory.rs`
- `crates/arbor-connectors/src/message.rs`

**Estimated Lines**: ~600

---

### Phase 3: Observability and Testing (1-2 weeks)

#### Week 9: Observability

**Priority**: 🟡 Important

**Tasks**:
1. OpenTelemetry integration
   - [ ] Tracing spans for check/list operations
   - [ ] Distributed tracing across indexer/authorizer
   - [ ] Trace export to Jaeger/Zipkin

2. Metrics
   - [ ] Prometheus metrics
   - [ ] Latency histograms (check, list, snapshot generation)
   - [ ] Counter metrics (permit, deny, errors)
   - [ ] Gauge metrics (snapshot version, policy count)

3. Structured logging
   - [ ] Use `tracing` crate throughout
   - [ ] JSON log format option
   - [ ] Log levels (trace, debug, info, warn, error)
   - [ ] Contextual logging (request IDs)

4. Dashboards
   - [ ] Example Grafana dashboards
   - [ ] Example Prometheus alerting rules

**Files to Create**:
- `crates/arbor-observability/src/lib.rs`
- `crates/arbor-observability/src/tracing.rs`
- `crates/arbor-observability/src/metrics.rs`
- `dashboards/grafana/arbor.json`
- `dashboards/prometheus/alerts.yml`

**Estimated Lines**: ~500

#### Week 9-10: Integration Testing

**Priority**: 🔴 Critical

**Tasks**:
1. End-to-end tests
   - [ ] Indexer → Authorizer flow
   - [ ] Snapshot notification propagation
   - [ ] Checksum verification
   - [ ] Snapshot recovery on failure

2. Load testing
   - [ ] k6 or Locust scripts
   - [ ] check() throughput benchmarks
   - [ ] list() latency benchmarks
   - [ ] Concurrent update + query scenarios

3. Correctness testing
   - [ ] Policy evaluation correctness suite
   - [ ] Hierarchical entity tests
   - [ ] Forbid precedence tests
   - [ ] Condition evaluation edge cases

**Files to Create**:
- `tests/integration/indexer_authorizer_test.rs`
- `tests/load/check_benchmark.js` (k6)
- `tests/load/list_benchmark.js`
- `tests/correctness/policy_evaluation.rs`

**Estimated Lines**: ~1,000

---

### Phase 4: Documentation and Examples (1 week)

#### Week 10-11: Documentation

**Priority**: 🟡 Important

**Tasks**:
1. User documentation
   - [ ] Getting started guide
   - [ ] API reference
   - [ ] Configuration reference
   - [ ] Deployment guide (Docker, Kubernetes)

2. Developer documentation
   - [ ] Contributing guide
   - [ ] Architecture deep dive
   - [ ] Code walkthrough
   - [ ] Testing guide

3. Examples
   - [ ] Simple file system authorization
   - [ ] Multi-tenant SaaS authorization
   - [ ] Healthcare RBAC example
   - [ ] Docker Compose deployment

4. README
   - [ ] Project overview
   - [ ] Quick start
   - [ ] Feature comparison (vs Cedar, OpenFGA, Ory Keto)
   - [ ] Performance benchmarks

**Files to Create**:
- `docs/getting-started.md`
- `docs/api-reference.md`
- `docs/configuration.md`
- `docs/deployment.md`
- `docs/contributing.md`
- `examples/file-system/README.md`
- `examples/saas/README.md`
- `examples/docker-compose.yml`

**Estimated Lines**: Documentation (prose, not counted)

---

### V1 Deliverables

By end of V1, Arbor will have:

- ✅ Complete bytecode VM with all OpCodes
- ✅ Snapshot builder with transitive closures
- ✅ All 4 authorization operations (check, list×3)
- ✅ Indexer service with broker pub/sub
- ✅ Authorizer service with dual transport (UDS + gRPC)
- ✅ Client libraries (Rust, Node.js, Python)
- ✅ Broker abstraction with NATS implementation
- ✅ OpenTelemetry observability
- ✅ Integration and load tests
- ✅ Documentation and examples

**Estimated Total Lines**: ~10,100 new lines + existing ~1,055 = **~11,155 lines**

**Note**: Delta support removed from V1 for simplicity (saves 2-3 weeks, deferred to V2+).

---

## V2: Production Hardening (8-12 weeks)

**Goal**: Enterprise-ready with HA, persistence, and advanced features.

### Phase 1: Persistence and HA (4-6 weeks)

1. **External Snapshot Storage**
   - [ ] S3/blob storage integration
   - [ ] Snapshot upload/download
   - [ ] Authorizers fetch from storage (not indexer)

2. **Indexer HA**
   - [ ] Leader election (etcd/Consul)
   - [ ] Hot standby indexers
   - [ ] Failover handling

3. **Database Integration**
   - [ ] PostgreSQL connector
   - [ ] MySQL connector
   - [ ] Change data capture (CDC)

4. **Incremental Indexing**
   - [ ] Incremental snapshot updates (not full rebuild)
   - [ ] Track dirty entities/policies
   - [ ] Optimize for small changes

### Phase 2: Advanced Features (2-3 weeks)

5. **InNetwork Condition Operator**
   - [ ] CIDR parsing
   - [ ] IP address matching
   - [ ] IPv4/IPv6 support

6. **Temporal Policies**
   - [ ] Time-based conditions (valid_after, valid_before)
   - [ ] Recurring schedules (business hours, weekdays)

7. **Policy Testing Framework**
   - [ ] Simulate authorization decisions
   - [ ] Policy coverage analysis
   - [ ] Test case generation

8. **Advanced Caching**
   - [ ] Redis cache for hot decisions
   - [ ] Cache invalidation on updates
   - [ ] TTL-based expiration

### Phase 3: Enterprise Features (2-3 weeks)

9. **Audit Logging**
   - [ ] Write all decisions to audit log
   - [ ] Structured audit events
   - [ ] Integration with SIEM systems

10. **Policy Versioning**
    - [ ] Track policy changes over time
    - [ ] Rollback to previous versions
    - [ ] Diff between versions

11. **Multi-Tenancy**
    - [ ] Namespace isolation
    - [ ] Per-tenant metrics
    - [ ] Tenant-specific policies

12. **Admin UI**
    - [ ] Web dashboard for policy management
    - [ ] Entity browser
    - [ ] Real-time metrics

---

## V3: Scale and Optimization (Future)

### Advanced Optimizations

1. **JIT Compilation**
   - Compile hot conditions to native code
   - 5-10x speedup for complex conditions
   - Platform-specific (x86_64, ARM64)

2. **SIMD Batch Evaluation**
   - Evaluate same condition for many resources
   - Vectorized operations

3. **Query Plan Optimization**
   - Cost-based query planning
   - Index selection
   - Join reordering

4. **Sharded Indexers**
   - Partition entities by namespace
   - Scale beyond single indexer capacity

### Advanced Features

5. **Policy Simulation**
   - "What-if" analysis
   - Impact assessment before deploying policies

6. **Machine Learning Integration**
   - Anomaly detection on authorization patterns
   - Policy recommendation
   - Access prediction

7. **Cross-Region Replication**
   - Multi-region deployments
   - Eventual consistency
   - Conflict resolution

---

## Dependencies

### Immediate (V1)

```toml
# Core
tokio = { version = "1.35", features = ["full"] }
uuid = { version = "1.6", features = ["v4", "v5"] }
serde = { version = "1.0", features = ["derive"] }

# Performance
roaring = "0.10"
rapid-hash = "0.1"
smallvec = "1.11"

# Serialization
bincode = "1.3"
prost = "0.12"  # For protobuf

# Pub/Sub
rdkafka = "0.35"  # Kafka
redis = "0.24"    # Redis Streams

# Observability
tracing = "0.1"
tracing-subscriber = "0.3"
metrics = "0.21"
metrics-exporter-prometheus = "0.13"

# HTTP/gRPC
tonic = "0.10"
axum = "0.7"
tower = "0.4"

# Testing
proptest = "1.4"
criterion = "0.5"
```

### Future (V2+)

```toml
# Persistence
aws-sdk-s3 = "1.0"
sqlx = { version = "0.7", features = ["postgres", "mysql"] }

# HA
etcd-client = "0.12"

# Advanced
ipnetwork = "0.20"  # CIDR parsing
regex = "1.10"
```

---

## Milestones

### Milestone 1: Core Engine (Week 5)
- ✅ Bytecode VM complete
- ✅ Snapshot builder working
- ✅ All 4 authorization operations implemented
- ✅ Unit tests passing

### Milestone 2: Services (Week 8)
- ✅ Indexer service running
- ✅ Authorizer service running with dual transport
- ✅ Client libraries published (Rust, Node.js, Python)
- ✅ Broker-based snapshot notifications working
- ✅ Integration tests passing

### Milestone 3: Observability (Week 9)
- ✅ Tracing enabled
- ✅ Metrics exposed
- ✅ Load tests complete
- ✅ Documentation written

### Milestone 4: V1 Release (Week 10)
- ✅ All V1 features complete
- ✅ Performance benchmarks meet targets (<1ms check, <10ms list)
- ✅ Client libraries published for all target languages
- ✅ Examples and documentation ready
- ✅ Docker images published

---

## Risk Mitigation

### Technical Risks

1. **Bytecode VM Performance**
   - Risk: Bytecode slower than expected
   - Mitigation: Benchmark early, have fallback to AST eval

2. **Snapshot Distribution**
   - Risk: Checksum mismatches, corrupt snapshots
   - Mitigation: Extensive testing, retry logic

3. **Broker Reliability**
   - Risk: Message loss, connection failures
   - Mitigation: Use reliable broker (NATS JetStream for persistence), implement retries

### Schedule Risks

1. **Underestimated Complexity**
   - Risk: Features take longer than estimated
   - Mitigation: Start with MVP, add features incrementally

2. **Dependencies**
   - Risk: Blocked on external dependencies
   - Mitigation: Use mocks for testing, implement stubs

---

## Success Metrics

### Performance (V1)

- [ ] check() p99 latency (sidecar/UDS): 30-80μs (Rust: 30-50μs, Node.js: 50-80μs)
- [ ] check() p99 latency (gRPC): 1-5ms
- [ ] list_resources() p99 latency: <10ms (10K resources)
- [ ] Snapshot generation: <200ms (10K entities, 1K policies)
- [ ] Snapshot update: <10ms
- [ ] Throughput: >10K check() ops/sec per authorizer (sidecar mode)

### Correctness (V1)

- [ ] 100% test coverage for core logic
- [ ] Property-based tests pass (bytecode ≡ AST)
- [ ] Forbid precedence never violated
- [ ] No false permits (security critical)

### Scalability (V1)

- [ ] Supports 100K entities
- [ ] Supports 10K policies
- [ ] Horizontal scaling: 10+ authorizers
- [ ] Indexer handles 100 updates/sec

### Observability (V1)

- [ ] All operations traced
- [ ] Metrics exposed in Prometheus format
- [ ] Structured logs
- [ ] Example dashboards

---

## Next Steps

1. **Immediate (This Week)**
   - Implement `BytecodeCompiler` (Condition AST → bytecode)
   - Begin snapshot builder (transitive closure computation)
   - Set up project structure for services

2. **Short-Term (Next 2 Weeks)**
   - Complete snapshot builder
   - Implement check() operation

3. **Medium-Term (Next Month)**
   - Complete all authorization operations
   - Build indexer service
   - Build authorizer service

4. **Long-Term (Next 3 Months)**
   - Complete V1
   - Deploy to production
   - Gather feedback for V2

---

## Related Documentation

- [Architecture](./architecture.md) - System design
- [Authorization Flow](./authorization-flow.md) - How operations work
- [Bytecode VM](./bytecode-vm.md) - VM implementation details
- [Snapshot Format](./snapshot-format.md) - Data structures
- [Data Model](./data-model.md) - Core types
