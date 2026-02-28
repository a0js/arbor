# YSNP to Arbor: Comparison and Migration

This document explains what changed from YSNP to Arbor, why those changes were made, and what functionality needs to be ported.

## Executive Summary

**YSNP** was a proof-of-concept authorization engine with all components (graph, indexes, evaluation) in a single process. It validated the core concepts but had scalability limitations.

**Arbor** is a production-grade redesign with:
- **Separate indexer and authorizer services** for horizontal scalability
- **Bytecode VM** for faster condition evaluation
- **Pub/sub delta distribution** for efficient updates
- **Cloud-native architecture** for containerized deployments

## Architectural Changes

### 1. Service Separation

**YSNP**:
```
┌─────────────────────────────────────┐
│   Authorization Engine (Single)     │
│                                      │
│  ┌────────┐  ┌─────────┐           │
│  │ Graph  │  │ Indexes │           │
│  │(RwLock)│  │(ArcSwap)│           │
│  └────────┘  └─────────┘           │
│                                      │
│  • Writes update graph               │
│  • snapshot() rebuilds indexes       │
│  • Reads query ArcSwap indexes       │
└─────────────────────────────────────┘
```

**Arbor**:
```
┌──────────────────┐            ┌──────────────────┐
│ Arbor Indexer    │            │ Arbor Authorizer │
│                  │   Pub/Sub  │   (many)         │
│ • Graph (write)  │  ───────>  │ • Snapshot (read)│
│ • Snapshot gen   │            │ • Evaluation     │
│ • Delta gen      │            │                  │
└──────────────────┘            └──────────────────┘
```

**Why Changed**:
- **Scalability**: Authorizers can scale horizontally without write contention
- **Separation of Concerns**: Indexing and evaluation are independent
- **Deployment Flexibility**: Authorizers can be sidecars or centralized
- **Lock Contention**: No RwLock contention in YSNP's single-process model

### 2. Condition Evaluation

**YSNP**:
```rust
fn evaluate_condition(condition: &Condition, context: &EvaluationContext) -> ConditionResult {
    match condition {
        Condition::And(l, r) => {
            let left_result = evaluate_condition(l, context);
            let right_result = evaluate_condition(r, context);
            // ... combine
        }
        Condition::Eq(l, r) => {
            let left_val = resolve_value(l, context);
            let right_val = resolve_value(r, context);
            // ... compare
        }
        // ... recursive AST traversal
    }
}
```
- Direct AST traversal
- Repeated pattern matching overhead
- Poor cache locality

**Arbor**:
```rust
// Compile once in indexer
let bytecode = compile_condition(condition);

// Interpret many times in authorizer
let result = bytecode_vm.evaluate(bytecode, context);
```
- Bytecode compilation amortizes parsing cost
- Stack-based evaluation (cache-friendly)
- 2-4x faster than AST traversal

**Why Changed**:
- **Performance**: Conditions evaluated millions of times, compilation cost paid once
- **Optimization**: Easier to optimize bytecode than AST
- **Future JIT**: Bytecode enables future JIT compilation

### 3. Update Distribution

**YSNP**:
```rust
// Update graph
graph.upsert_entity(entity)?;

// Rebuild all indexes
let new_indexes = rebuild_indexes(&graph);

// Atomic swap
indexes.store(Arc::new(new_indexes));
```
- Full snapshot rebuild on every change
- In-process atomic swap via ArcSwap
- No distribution to other processes

**Arbor**:
```rust
// Update graph (indexer)
graph.upsert_entity(entity)?;

// Generate new snapshot
let new_snapshot = generate_snapshot(&graph);

// Compute delta from previous
let delta = compute_delta(&prev_snapshot, &new_snapshot);

// Publish delta via pub/sub
pub_sub.publish(delta);
```
- Incremental deltas (not full snapshots)
- Distributed to many authorizers via pub/sub
- Authorizers apply deltas with checksum verification

**Why Changed**:
- **Efficiency**: Deltas are smaller than full snapshots (KB vs MB)
- **Distribution**: Must propagate updates to multiple authorizer instances
- **Hot Swapping**: Authorizers update with zero downtime

### 4. Storage Model

**YSNP**:
- In-memory only
- No persistence layer implemented
- `ysnp-sync` crate was a stub

**Arbor V1**:
- In-memory snapshots
- External persistence on roadmap (V2)
- Focus on distribution, not persistence

**Why Changed**:
- **Pragmatism**: YSNP never implemented persistence either, so defer to V2
- **Container-Native**: Ephemeral containers, don't rely on local disk
- **External Storage**: When added, will use S3/blob storage, not local files

## What Was Kept from YSNP

### ✅ Core Concepts

1. **Graph-Based Entity Model**
   - Hierarchical entities with multiple parents
   - Circular dependency detection
   - Transitive closures (ancestors/descendants)

2. **Policy Model**
   - Permit/Forbid semantics
   - PolicyTarget enum (Entity, EntityWithDescendants, EntityType, All)
   - Conditional policies with attribute-based access control

3. **Roaring Bitmap Indexes**
   - Entity type → entities
   - Action → policies
   - Principal → policies
   - Resource → policies
   - Specialized bitmaps (conditional, forbidding, descendants)

4. **Authorization Operations**
   - check()
   - list_resources()
   - list_principals()
   - list_actions()

5. **Evaluation Semantics**
   - Forbid takes precedence
   - Default deny
   - Short-circuit evaluation
   - Three-valued logic (True/False/Unknown/Invalid)

6. **Attribute System**
   - Nested objects with path-based access
   - Typed scalars (String, Integer, Float, Bool, Timestamp)
   - Entity references and sets

### ✅ Performance Optimizations

1. **U32 Internal Indexing**
   - UUID mapping to compact indices
   - Cache-friendly sequential access

2. **Two-Phase Listing**
   - Bitmap operations for unconditional policies
   - Conditional evaluation only for residuals

3. **Attribute Shape Caching**
   - Cache condition results by attribute structure
   - Avoid re-evaluating same condition for similar entities

4. **Precomputed Relationships**
   - Ancestors/descendants computed once
   - No graph traversal at query time

## What Changed from YSNP

### 🔄 Modified

1. **Service Architecture**
   - YSNP: Single process with RwLock + ArcSwap
   - Arbor: Separate indexer and authorizer services

2. **Condition Evaluation**
   - YSNP: Direct AST traversal
   - Arbor: Bytecode compilation + interpretation

3. **Update Propagation**
   - YSNP: In-process snapshot swap
   - Arbor: Pub/sub delta distribution

4. **String IDs**
   - YSNP: Basic string interning with `StringInterner`
   - Arbor: Type-safe `StringId<T>` with phantom types

5. **Snapshot Generation**
   - YSNP: `rebuild_indexes()` on demand
   - Arbor: Versioned snapshots with deltas and checksums

### ➕ Added to Arbor

1. **Bytecode VM**
   - OpCode instruction set
   - Stack-based interpreter
   - Compilation in indexer

2. **Delta System**
   - DeltaPackage with operations
   - Checksum verification
   - Incremental updates

3. **Pub/Sub Protocol**
   - Snapshot version announcements
   - Delta distribution
   - Full snapshot fallback

4. **Hybrid Action Model**
   - Actions can be type-scoped or global
   - YSNP only had type-scoped actions

5. **Explainability**
   - Optional reasons for decisions
   - Debug mode for troubleshooting

6. **Observability**
   - OpenTelemetry tracing integration
   - Structured logging
   - Metrics for latency and policy evaluation

## What's Missing in Arbor (Needs Porting)

### 🔴 Critical (V1 Blockers)

1. **Authorization Check Logic**
   - **File**: `ysnp-core/src/authorization_engine.rs`
   - **Lines**: ~200-300
   - **Port to**: `arbor-authorizer/src/check.rs`
   - **Changes**: Use bytecode VM instead of AST evaluator

2. **List Operations**
   - **Files**:
     - `list_resources()` in `authorization_engine.rs`
     - `list_principals()` in `authorization_engine.rs`
     - `list_actions()` in `authorization_engine.rs`
   - **Port to**: `arbor-authorizer/src/list.rs`
   - **Changes**: Minimal, mostly direct port

3. **Snapshot Builder**
   - **File**: `ysnp-core/src/store/indexes.rs` (`rebuild_indexes()`)
   - **Lines**: ~500
   - **Port to**: `services/arbor-indexer/src/snapshot_builder.rs`
   - **Changes**: Add bytecode compilation step

4. **Condition Evaluator → Bytecode VM**
   - **File**: `ysnp-core/src/conditions/evaluator.rs`
   - **Port to**: `arbor-bytecode/src/vm.rs`
   - **Changes**: Complete rewrite as bytecode interpreter

5. **Attribute Path Resolution**
   - **File**: `ysnp-core/src/store/attributes.rs`
   - **Port to**: `arbor-types/src/attributes.rs`
   - **Changes**: Helper method on `Attributes` type

6. **Action/ActionSet Management**
   - **File**: `ysnp-core/src/graph/graph.rs`
   - **Port to**: `arbor-graph-core/src/mutations.rs`
   - **Changes**: Add full CRUD for actions/action sets

### 🟡 Important (V1 Nice-to-Have)

7. **Policy Split Helper**
   - **File**: `ysnp-core/src/store/indexes.rs` (`split_policy_map_for_authorization()`)
   - **Port to**: `arbor-index-snapshot/src/lib.rs`
   - **Changes**: None, direct port

8. **Transitive Closure Computation**
   - **File**: `ysnp-core/src/store/indexes.rs` (closure algorithms)
   - **Port to**: `services/arbor-indexer/src/snapshot_builder.rs`
   - **Changes**: None, direct port

9. **String Interning**
   - **File**: `ysnp-core/src/store/interners.rs`
   - **Status**: Arbor has `StringId<T>` but no interner implementation
   - **Decision**: May not be needed, `StringId` might be sufficient

### 🟢 Optional (V2+)

10. **Policy Dependency Computation**
    - **File**: `ysnp-core/src/conditions/mod.rs` (`compute_dependencies()`)
    - **Port to**: `arbor-types/src/conditions.rs`
    - **Use**: Optimization (load only needed attributes)

11. **Entity Type Registry**
    - **File**: `ysnp-core/src/types/entity_type_registry.rs`
    - **Status**: Not implemented in YSNP either
    - **Need**: Validation that entity types exist

## Code Size Comparison

| Component | YSNP (lines) | Arbor (lines) | Status |
|-----------|--------------|---------------|--------|
| Core types | ~400 | ~600 | ✅ Complete |
| Graph storage | ~800 | ~700 | ✅ Complete |
| Indexes | ~600 | ~400 | 🚧 Structure done, builder missing |
| Condition eval | ~300 | ~0 | ❌ Needs bytecode VM |
| Auth operations | ~500 | ~0 | ❌ Not implemented |
| Bytecode VM | ~0 | ~50 | 🚧 Stub only |
| Services | ~0 (stubs) | ~0 (stubs) | ❌ Not implemented |
| **Total** | **~4,463** | **~1,055** | **~24% complete** |

## Migration Checklist

### Phase 1: Core Functionality (V1)

- [ ] Complete bytecode VM implementation
- [ ] Port snapshot builder from YSNP
- [ ] Implement check() operation
- [ ] Implement list_resources()
- [ ] Implement list_principals()
- [ ] Implement list_actions()
- [ ] Add attribute path resolution
- [ ] Complete action/action set management
- [ ] Implement delta generation
- [ ] Implement delta application

### Phase 2: Services (V1)

- [ ] arbor-indexer service skeleton
- [ ] Connector interface (read from source data)
- [ ] Pub/sub publisher in indexer
- [ ] arbor-authorizer service skeleton
- [ ] Pub/sub subscriber in authorizer
- [ ] gRPC/HTTP API in authorizer

### Phase 3: Testing (V1)

- [ ] Port YSNP unit tests
- [ ] Add bytecode VM tests
- [ ] Integration tests (indexer + authorizer)
- [ ] Performance benchmarks
- [ ] Correctness verification

### Phase 4: Observability (V1)

- [ ] OpenTelemetry tracing
- [ ] Structured logging
- [ ] Metrics (latency, policy hits, etc.)
- [ ] Health checks

### Phase 5: Production Hardening (V2)

- [ ] External snapshot persistence (S3)
- [ ] HA indexer (leader election)
- [ ] Advanced error handling
- [ ] Circuit breakers
- [ ] Rate limiting

## Performance Comparison

### Expected Improvements Over YSNP

| Operation | YSNP | Arbor (Expected) | Improvement |
|-----------|------|------------------|-------------|
| check() (no conditions) | <1ms | <1ms | Same |
| check() (with conditions) | 1-5ms | 0.5-2ms | 2-3x faster |
| list_resources() (10K) | 5-20ms | 5-20ms | Same |
| Snapshot generation | 50-200ms | 50-200ms | Same |
| Update propagation | In-process | Network | Different model |

**Key Improvements**:
- Condition evaluation: 2-4x faster (bytecode vs AST)
- Horizontal scaling: Unlimited authorizers (YSNP: single instance)
- Deployment flexibility: Sidecars or centralized (YSNP: monolithic)

### Trade-offs

**Arbor Advantages**:
- Horizontal scalability
- Separate read/write paths
- Zero authorizer downtime on updates
- Flexible deployment models

**YSNP Advantages**:
- Simpler architecture (single process)
- No network overhead for updates
- Easier to debug (everything in one place)

## API Compatibility

### YSNP API

```rust
impl AuthorizationEngine {
    async fn check(principal_id, action_id, resource_id) -> AuthorizationResult;
    async fn list_resources(principal_id, action_id, resource_type) -> ListingResult<Entity>;
    async fn list_principals(action_id, resource_id, principal_type) -> ListingResult<Entity>;
    async fn list_actions(principal_id, resource_id) -> ListingResult<Action>;
}
```

### Arbor API (Planned)

```rust
impl ArborAuthorizer {
    async fn check(req: CheckRequest) -> CheckResponse;
    async fn list_resources(req: ListResourcesRequest) -> ListResourcesResponse;
    async fn list_principals(req: ListPrincipalsRequest) -> ListPrincipalsResponse;
    async fn list_actions(req: ListActionsRequest) -> ListActionsResponse;
}
```

**Changes**:
- Request/Response structs instead of positional arguments
- Added `context` parameter (for ABAC attributes)
- Added `explain` flag (for debugging)
- Added `limit`/`offset` for pagination

**Compatibility**: Easy to wrap Arbor API to match YSNP API if needed.

## Lessons Learned from YSNP

### What Worked Well

1. **Roaring Bitmaps**: Excellent choice for set operations
2. **Precomputed Closures**: Critical for performance
3. **Two-Phase Listing**: Clever optimization
4. **Forbid Precedence**: Clear security semantics
5. **Hierarchical Entities**: Powerful and flexible

### What Didn't Work

1. **Single Process**: Can't scale horizontally
2. **No Persistence**: Lost state on restart
3. **AST Evaluation**: Too slow for hot paths
4. **Full Snapshot Rebuilds**: Wasteful for small changes
5. **No Distribution**: Can't deploy as sidecars

### Applied to Arbor

- ✅ Keep bitmap indexes
- ✅ Keep precomputed closures
- ✅ Keep two-phase listing
- ✅ Keep forbid precedence
- ✅ Keep hierarchical entities
- ➕ Add bytecode VM
- ➕ Add delta system
- ➕ Add service separation
- ➕ Add pub/sub distribution
- ⏭️ Defer persistence to V2

## Related Documentation

- [Architecture](./architecture.md) - Arbor's design
- [Authorization Flow](./authorization-flow.md) - How operations work
- [Bytecode VM](./bytecode-vm.md) - New evaluation engine
- [Snapshot Format](./snapshot-format.md) - Versioned snapshots and deltas
- [Implementation Roadmap](./implementation-roadmap.md) - What to build next
