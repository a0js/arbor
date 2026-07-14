# Arbor Architecture

## Overview

Arbor is a high-performance, graph-based authorization system designed for:
- **Hierarchical entity relationships** with directed acyclic graphs (DAGs)
- **Attribute-based access control (ABAC)** with rich conditional policies
- **Separate read/write paths** for optimal performance and scalability
- **Horizontally scalable authorization** with stateless decision engines
- **Bytecode compilation** for fast policy condition evaluation

## Design Philosophy

1. **Correctness First**: Authorization decisions must be accurate. Speed is important, but never at the cost of correctness.
2. **Deny by Default**: Explicit permission required for all operations.
3. **Forbid Takes Precedence**: Security over convenience - a single forbid overrides any permits.
4. **Separation of Concerns**: Write path (indexer) and read path (authorizer) are independent.
5. **Cloud-Native**: Designed for containerized, distributed deployments.

## High-Level Architecture

```
┌─────────────┐
│ Source Data │ (Database, CSV, API)
└──────┬──────┘
       │
       │ Connectors (arbor-connectors)
       ▼
┌──────────────────┐
│ Arbor Indexer    │ ─── Builds snapshots from source data
│                  │ ─── Compiles conditions to bytecode
│  - Graph Store   │ ─── Generates deltas between versions
│  - Snapshot Gen  │ ─── Publishes updates via pub/sub
│  - Bytecode VM   │ ─── Serves snapshots via gRPC/HTTP
└────────┬─────────┘
         │
         │ Pub/Sub (Kafka, Redis, NATS, etc.)
         │
    ┌────┴────┐
    ▼         ▼         ▼
┌──────┐  ┌──────┐  ┌──────┐
│Auth  │  │Auth  │  │Auth  │  Arbor Authorizers (many instances)
│ -1   │  │ -2   │  │ -N   │  - Subscribe to snapshot updates
└──────┘  └──────┘  └──────┘  - Apply deltas incrementally
    │         │         │      - Evaluate authorization requests
    │         │         │      - Stateless after snapshot load
    ▼         ▼         ▼
┌─────────────────────────┐
│   Application Layer     │
│  (Microservices, APIs)  │
└─────────────────────────┘
```

## Components

### Core Crates

#### arbor-types
**Purpose**: Foundation type definitions shared across all components.

**Provides**:
- `Entity`: Principals, resources, or both with hierarchical relationships
- `Policy`: Authorization rules with permit/forbid semantics
- `Action`: Operations that can be performed (type-scoped or global)
- `ActionSet`: Groups of actions (role-like concept)
- `Attributes`: Nested key-value store for entity properties
- `Condition`: Rich boolean expressions for policy conditions
- `ByteCode`: OpCode definitions for compiled conditions
- Type-safe IDs using phantom types (`EntityTypeId`, `AttributeNameId`)

**Dependencies**: None (foundation crate)

#### arbor-graph-core
**Purpose**: Mutable graph storage for entities, policies, and relationships.

**Provides**:
- Dense array storage with u32 indices
- Parent-child bidirectional relationships
- CRUD operations with validation
- Circular dependency detection (DFS-based)
- Free list management for deleted nodes
- Referential integrity enforcement

**Key Operations**:
- `upsert_entity()`: Insert/update entities with cycle detection
- `upsert_policy()`: Add policies with target validation
- `add_action()`, `upsert_action_set()`: Action management
- `remove_*()`: Clean deletion with relationship cleanup

**Design**: This is the **authoritative source of truth** during indexing. The graph is optimized for writes and validation, not for reads.

#### arbor-index-snapshot
**Purpose**: Read-optimized indexes derived from the graph for fast authorization queries.

**Provides**:
- `IndexSnapshot`: Immutable snapshot of authorization state
- Precomputed ancestor/descendant bitmaps
- Multi-dimensional policy indexes (principal × resource × action × type)
- Specialized bitmaps for query optimization
- Policy categorization (unconditional/conditional, permit/forbid)

**Key Query Methods**:
- `get_policies_for_resource()`: All policies applicable to a resource
- `get_policies_for_principal()`: All policies applicable to a principal
- `get_policies_for_action()`: Policies for a specific action
- `split_policy_map_for_authorization()`: Categorize policies for evaluation

**Design**: This is the **read-optimized representation**. The snapshot is built by the indexer and consumed by authorizers. It's immutable and safe for concurrent access.

#### arbor-bytecode
**Purpose**: Stack-based bytecode VM for evaluating compiled policy conditions.

**Provides**:
- `ByteCodeVM`: Interpreter for bytecode instructions
- `EvaluationContext`: Principal, resource, action, and context attributes
- Stack-based evaluation for simplicity and speed
- Variable resolution from scopes (Principal, Resource, Context)

**OpCodes** (defined in arbor-types):
- Stack: `PushScalar`, `PushEntityRef`, `PushVariable`
- Comparison: `Eq`, `Neq`, `Lt`, `Lte`, `Gt`, `Gte`
- Logical: `And`, `Or`, `Not`
- Set operations: `In`, `Contains`, `ContainsAll`, `ContainsAny`
- Attributes: `HasAttribute`
- Control flow: `JumpIfFalse`, `Jump`

**Compilation Strategy**: Conditions are compiled to bytecode in the **indexer** during snapshot generation. The bytecode is serialized and included in the snapshot. Authorizers receive pre-compiled bytecode and only need to interpret it.

**Why Bytecode?**
- Amortizes parsing cost (compile once, run many times)
- Better cache locality than AST traversal
- Enables future optimizations (constant folding, dead code elimination)
- Language-agnostic format

#### arbor-connectors
**Purpose**: External system integrations for data ingestion.

**Status**: 🚧 In progress (V1)

**V1 Functionality**:
- `ConnectorsConfig` and `EntityTypesConfig` — YAML-driven configuration types
- `Connector` trait: `load(&self) -> ArborResult<Graph>`
- `PostgresConnector`: runs user-defined SQL queries per entity type, assembles a `Graph`
- `ExampleConnector`: hardcoded graph for dev/test

**Configuration**: Two YAML files — `config/connectors.yaml` (connection credentials) and `config/entity_types.yaml` (SQL queries per entity type referencing connectors by name). See [connectors.md](./connectors.md) for full details.

**V2+**:
- CDC connectors (Debezium, database triggers)
- Streaming connectors (Kafka, Kinesis, NATS)
- Additional databases (MySQL, DynamoDB)

#### arbor-proto-internal
**Purpose**: Internal protocol definitions for service communication.

**Status**: 🚧 Placeholder

**Intended Functionality**:
- gRPC service definitions
- Protobuf message types
- Client/server stubs

### Services

#### arbor-indexer
**Purpose**: Transform source data into optimized authorization indexes.

**Responsibilities**:
1. **Ingest Data**: Pull entities, policies, actions from source via connectors
2. **Build Graph**: Populate mutable graph with validation
3. **Generate Snapshots**: Create read-optimized indexes with precomputed relationships
4. **Compile Conditions**: Convert policy conditions to bytecode
5. **Batch Updates**: Debounce/batch graph changes before generating snapshots
6. **Publish Updates**: Broadcast new snapshot availability via pub/sub
7. **Serve Snapshots**: Provide full snapshots to authorizers via gRPC/HTTP

**Boot Sequence**:
```
1. Initialize graph and storage
2. Fetch current state from data source (full load)
3. Build initial snapshot (v1)
4. Start pub/sub publisher
5. Start gRPC/HTTP server
6. Start listening for source data changes
7. On changes: Batch updates → Generate snapshot v(n+1) → Publish availability
```

**Snapshot Generation Strategy (V1)**:
- **Batching**: Updates are batched before triggering snapshot generation (TBD: time-based, count-based, or debounced)
- **Monotonic versions**: Each snapshot has incrementally increasing version number (u64)
- **Checksum verification**: SHA256 checksum for integrity
- **Change detection**: Authorizers compare checksums to skip unnecessary downloads

**Storage (V1)**: In-memory only
- On restart: Full rebuild from source data
- Cold start time: Acceptable for v1 (seconds to minutes depending on data size)
- Trade-off: Simplicity vs recovery time

**Storage (V2+)**: External persistence
- Snapshots stored in S3/blob storage
- Fast recovery on restart
- Enables true HA with multiple indexers

#### arbor-authorizer
**Purpose**: Stateless authorization decision service.

**Responsibilities**:
1. **Subscribe to Snapshots**: Connect to indexer pub/sub, receive availability notifications
2. **Fetch Snapshots**: Download full snapshots when updates available
3. **Verify Checksums**: Ensure snapshot integrity
4. **Handle Check Requests**: "Can principal X perform action Y on resource Z?"
5. **Handle List Requests**: "What resources can principal X access?"
6. **Evaluate Conditions**: Execute bytecode for conditional policies
7. **Return Decisions**: With optional explanations (debug mode)

**API Operations** (see [Authorization Flow](./authorization-flow.md)):
- `check(principal_id, action_id, resource_id, context, explain)` → Decision + optional reason
- `list_resources(principal_id, action_id, resource_type, context)` → List of resources
- `list_principals(action_id, resource_id, principal_type, context)` → List of principals
- `list_actions(principal_id, resource_id, context)` → List of actions

**Snapshot Update Process (V1)**:
1. Receive pub/sub message: "Snapshot v123 available, checksum=ABC"
2. Compare checksum with current snapshot
3. If different: Fetch full snapshot from indexer (gRPC/HTTP)
4. Verify checksum
5. Atomically swap snapshot pointer
6. Continue serving requests with new snapshot

**Stale Data Handling**:
- If indexer is down, authorizers serve with last known snapshot
- Trade-off: Availability vs freshness (configurable policy)
- Responses include snapshot version for debugging

**Scalability**:
- Horizontally scalable (add more instances)
- Stateless after snapshot load
- No coordination between authorizers required
- Can run as sidecars or centralized service

## Data Flow

### Write Path (Indexing)

```
1. Source Data Change
   ↓
2. Connector detects change (Kafka event, database trigger, polling)
   ↓
3. Indexer receives change notification
   ↓
4. Indexer updates graph (upsert_entity, upsert_policy, etc.)
   ↓
5. Indexer triggers snapshot generation
   ↓
6. Snapshot builder:
   - Computes transitive closures (ancestors/descendants)
   - Compiles conditions to bytecode
   - Builds policy indexes (principal, resource, action, type)
   - Populates specialized bitmaps
   ↓
7. Delta generator compares v(n) and v(n+1), creates delta operations
   ↓
8. Indexer computes checksums for snapshot and delta
   ↓
9. Indexer publishes to pub/sub:
   - "Snapshot v(n+1) available"
   - Delta operations
   - Checksums
   ↓
10. Indexer stores snapshot in memory (and optionally external storage in v2+)
```

### Read Path (Authorization)

```
1. Client sends authorization request to Authorizer
   ↓
2. Authorizer looks up entities by UUID
   ↓
3. Authorizer gets applicable policies:
   - For principal (direct, ancestral, type-based, all)
   - For resource (direct, ancestral, type-based, all)
   - For action
   - Intersect the three sets
   ↓
4. Authorizer splits policies into 4 categories:
   - Unconditional forbid
   - Conditional forbid
   - Unconditional permit
   - Conditional permit
   ↓
5. Evaluation (short-circuit):
   a. If any unconditional forbid → DENY (with reason)
   b. Evaluate conditional forbids → if any true → DENY (with reason)
   c. If any unconditional permit AND no conditional forbids → ALLOW (with reason)
   d. Evaluate conditional permits → if any true → ALLOW (with reason)
   e. Default → DENY (no applicable policies)
   ↓
6. Return decision + optional explanation (if explain=true)
```

### Update Path (Delta Application)

```
1. Authorizer receives pub/sub message: "Snapshot v101 available"
   ↓
2. Authorizer checks current version (e.g., v100)
   ↓
3. Authorizer requests delta v100→v101 from indexer
   ↓
4. Authorizer applies delta operations:
   - EntityAdded → Add to uuid_to_index, indexed_entities
   - EntityRemoved → Remove from indexes
   - PolicyAdded → Add to indexed_policies, update bitmaps
   - PolicyRemoved → Remove from indexes, update bitmaps
   - etc.
   ↓
5. Authorizer computes checksum of resulting snapshot
   ↓
6. If checksum matches delta.expected_checksum:
   - Atomically swap to new snapshot
   - Continue serving requests
   ↓
7. If checksum fails:
   - Log error
   - Request full snapshot v101 from indexer
   - Apply full snapshot (may cause brief latency spike)
```

## Performance Optimizations

### Transport-Level Optimizations

**Unix Domain Sockets (Sidecar Mode)**:
- **No network stack**: Direct inter-process communication
- **Nagle disabled**: `TCP_NODELAY` for immediate sends (saves 10-40ms)
- **Abstract sockets** (Linux): ~10μs faster than filesystem sockets
- **Connection pooling**: Reuse connections (saves 50-100μs per request)
- **Length-prefixed Protobuf**: Efficient binary protocol
- **Result**: 30-80μs end-to-end latency

**gRPC/HTTP2 (Centralized Mode)**:
- **Multiplexed streams**: Multiple requests per connection
- **Binary protocol**: Protobuf over HTTP/2
- **Connection pooling**: Persistent connections
- **Result**: 1-5ms latency (network-dependent)

### Data Structure Optimizations

**Roaring Bitmaps**:
- Compressed bitmap operations for set logic
- Fast intersection/union operations (O(n) in compressed size)
- Used for: descendants, ancestors, policy sets
- Typical compression: 10-100x vs naive bitmaps

**U32 Indexing**:
- 4-byte references instead of 16-byte UUIDs internally
- Cache-friendly sequential access
- UUID mapping maintained separately

**Precomputed Relationships**:
- Transitive closures computed once during indexing
- Ancestors/descendants available as bitmaps
- No graph traversal at query time

### Evaluation Optimizations

**Bytecode Compilation**:
- Conditions compiled once in indexer
- Authorizers only interpret (no parsing overhead)
- Stack-based VM for simplicity and speed
- 2-4x faster than AST evaluation

**Multi-Dimensional Indexing**:
- Policies indexed by: principal, resource, action, type
- Specialized bitmaps for: all entities, descendants, conditionals, forbids
- Fast policy lookup: bitmap intersection instead of graph traversal

**Type-Safe IDs**:
- Phantom types prevent ID confusion at compile time
- String interning for entity types and attribute names
- Memory-efficient repeated strings

### Client-Side Optimizations

**Connection Reuse**:
- Single long-lived connection per client
- Avoid reconnection overhead (50-100μs per request)
- Automatic reconnection on failure

**Request Pipelining** (optional):
- Send multiple requests without waiting
- Amortizes socket overhead
- Responses matched by request ID

**Protocol Efficiency**:
- Protobuf: 2-5x smaller than JSON
- Binary serialization: 2-10μs vs 10-50μs for JSON
- Fixed-size buffers: Avoid allocations

## Correctness Guarantees

### Circular Dependency Detection
- DFS-based cycle detection on entity updates
- Prevents infinite loops in hierarchy traversal
- Maintains graph as a DAG (directed acyclic graph)

### Referential Integrity
- Policy validation ensures targets exist
- Actions and action sets must exist before reference
- Prevents dangling references

### Checksum Verification
- SHA256 checksums for every snapshot and delta
- Authorizers verify integrity after delta application
- Fallback to full snapshot on verification failure

### Atomic Updates
- Snapshot swaps are atomic (pointer swap)
- Requests either see old snapshot or new snapshot, never partial state
- No torn reads

### Type Safety
- Rust's type system enforces correctness at compile time
- Phantom types prevent mixing different ID types
- Enum exhaustiveness checks ensure all cases handled

## Scalability Strategy

### Horizontal Scaling (Authorizers)
- Add more authorizer instances as needed
- No coordination required between instances
- Linear scalability for read operations

### Vertical Scaling (Indexer)
- Single indexer can handle large datasets (tested: 50K entities, 1M policies)
- Memory usage: O(entities + policies + relationships)
- CPU usage: Dominated by snapshot generation (periodic, not per-request)

### HA Strategy (V1)
- Multiple authorizers (stateless, easy to scale)
- Single indexer (acceptable SPOF for v1)
- Authorizers tolerate indexer downtime (serve stale data)

### HA Strategy (V2+)
- External snapshot storage (S3, blob storage)
- Multiple indexers with leader election
- Fast recovery on indexer failure
- Authorizers fetch from external storage

## Deployment Models

### Sidecar Model (Ultra-Low Latency)
```
┌─────────────────────────────────────────────┐
│   Application Pod/Host                      │
│                                             │
│   ┌────────────────┐    ┌────────────────┐ │
│   │ Application    │    │ Authorizer     │ │
│   │ (any language) │    │ (Sidecar)      │ │
│   │                │    │                │ │
│   │  ┌──────────┐  │    │ • Snapshot     │ │
│   │  │  Arbor   │  │    │ • Bytecode VM  │ │
│   │  │  Client  │◄─┼────┤ • UDS Server   │ │
│   │  │  Lib     │  │UDS │                │ │
│   │  └──────────┘  │    └────────────────┘ │
│   └────────────────┘    30-80μs latency    │
└─────────────────────────────────────────────┘
```
**Transport**: Unix Domain Socket + Protobuf
- **Ultra-low latency**: 30-80μs per check()
- **Process isolation**: Separate containers/processes
- **No network hops**: Unix socket on same host
- **Language agnostic**: Client libraries for all languages
- **Memory efficient**: One snapshot per host (shared by authorizer)

### Centralized Model (Network)
```
┌──────────┐   ┌──────────┐   ┌──────────┐
│ Service  │   │ Service  │   │ Service  │
│    A     │   │    B     │   │    C     │
│          │   │          │   │          │
│  Arbor   │   │  Arbor   │   │  Arbor   │
│  Client  │   │  Client  │   │  Client  │
└────┬─────┘   └────┬─────┘   └────┬─────┘
     │              │              │
     │         gRPC / HTTP/2        │
     │         1-5ms latency        │
     └──────────────┼───────────────┘
                    │
              ┌─────▼──────┐
              │ Authorizer │
              │   Cluster  │
              │  (gRPC)    │
              └────────────┘
```
**Transport**: gRPC/HTTP2 + Protobuf
- **Network-capable**: Cross-host, multi-region
- **Shared resource pool**: Easier to manage
- **Higher latency**: 1-5ms (network overhead)
- **Use case**: Development, testing, non-critical paths

### Hybrid Model (Best of Both)
```
┌─────────────────────────┐   ┌─────────────────────────┐
│   Critical Service      │   │   Standard Service      │
│   ┌─────────────────┐   │   │                         │
│   │ Application     │   │   │   Application           │
│   │                 │   │   │                         │
│   │ Arbor Client ◄──┼───┼───┤ Arbor Client            │
│   └────────┬────────┘   │   └───────────┬─────────────┘
│            │ UDS        │               │ gRPC
│            │ 30-80μs    │               │ 1-5ms
│   ┌────────▼────────┐   │               │
│   │ Authorizer      │   │               │
│   │ (Sidecar)       │   │               │
│   └─────────────────┘   │               │
└─────────────────────────┘               │
                                    ┌─────▼──────┐
                                    │ Authorizer │
                                    │   Cluster  │
                                    └────────────┘
```
**Dual Transport**:
- Critical services: UDS sidecar (30-80μs)
- Standard services: gRPC centralized (1-5ms)
- **Same client library**: Auto-detects transport mode
- **No code changes**: Configuration-driven deployment

## Client-Server Communication

### Dual Transport Architecture

Arbor supports **two transport mechanisms** using the **same Protobuf protocol**:

```
┌─────────────────────────────────────────────────┐
│        Protocol Definition (.proto)             │
│                                                 │
│  service AuthorizationService {                │
│    rpc Check(CheckRequest) returns (CheckResponse);
│    rpc ListResources(...) returns (...);       │
│  }                                              │
└─────────────────┬───────────────────────────────┘
                  │
        ┌─────────┴──────────┐
        │                    │
        ▼                    ▼
┌───────────────┐    ┌──────────────────┐
│ Unix Socket   │    │ gRPC/HTTP2       │
│ + Protobuf    │    │ + Protobuf       │
│               │    │                  │
│ • Sidecar     │    │ • Network        │
│ • 30-80μs     │    │ • 1-5ms          │
│ • Same host   │    │ • Cross-host     │
└───────────────┘    └──────────────────┘
```

### Transport Selection

**Authorizer startup modes**:

```toml
# arbor-authorizer.toml
[server]
mode = "both"  # "sidecar", "centralized", or "both"

[server.unix_socket]
enabled = true
path = "/var/run/arbor.sock"
abstract = true  # Try abstract socket first (Linux, ~10μs faster)

[server.grpc]
enabled = true
address = "0.0.0.0:8080"
```

**Client auto-detection**:

```javascript
// Client library auto-detects best transport
const client = new ArborClient(); // Checks for /var/run/arbor.sock

// If socket exists → UDS (30-80μs)
// If not → gRPC to configured address (1-5ms)
```

### Unified Client Libraries

**One library per language**, supports both transports:

```
Language Support:
├── Rust:      arbor-client (crates.io)
├── Node.js:   @arbor/client (npm)
├── Python:    arbor-client (PyPI)
├── Go:        github.com/arbor/arbor-go
└── Java:      com.arbor.client (Maven)

Each library:
  ✅ Supports both UDS and gRPC
  ✅ Auto-detects transport mode
  ✅ Same API for both
  ✅ Generated from .proto files
  ✅ Connection pooling/reuse
```

**Benefits**:
- No code changes when switching deployment models
- Same API regardless of transport
- Performance optimized automatically based on deployment
- Easy to test both modes

## Technology Choices

### Rust
- Memory safety without garbage collection
- Zero-cost abstractions
- Excellent performance
- Strong type system for correctness

### Roaring Bitmaps
- Industry-standard compressed bitmaps
- Used by Elasticsearch, Pilosa, etc.
- Proven at scale

### Pub/Sub
- Flexible: Kafka, Redis, NATS, RabbitMQ, MQTT
- Decouples indexer from authorizers
- Natural fit for event-driven updates

### gRPC/HTTP
- gRPC for low-latency internal communication
- HTTP for external APIs and flexibility
- Protobuf for efficient serialization

## Future Enhancements (V2+)

### External Snapshot Persistence
- S3/blob storage for snapshots
- Fast recovery on restart
- True HA with multiple indexers

### Advanced HA
- Active-active indexers with versioning
- Sharded indexers for massive scale
- Cross-region replication

### Query Optimizations
- Incremental index updates (no full rebuild)
- Query result caching
- Query plan optimization

### Advanced Features
- What-if analysis ("what would change if...")
- Policy simulation and testing
- InNetwork condition operator (IP ranges)
- Temporal policies (time-based access)

### Observability Enhancements
- Distributed tracing (OpenTelemetry)
- Detailed metrics (query latency, policy hit rates)
- Policy coverage analysis
- Authorization dashboards

## Version History

- **V1 (Current)**: In-memory snapshots, single indexer, basic pub/sub, bytecode VM
- **V2 (Planned)**: External persistence, HA indexers, advanced observability
- **V3 (Future)**: Advanced features, sharding, cross-region

## Related Documentation

- [Authorization Flow](./authorization-flow.md) - How check and list operations work
- [Pub/Sub Protocol](./pub-sub-protocol.md) - Delta distribution protocol
- [Snapshot Format](./snapshot-format.md) - Snapshot and delta structure
- [Bytecode VM](./bytecode-vm.md) - Condition compilation and evaluation
- [Data Model](./data-model.md) - Entities, policies, actions, attributes
- [Deployment Models](./deployment-models.md) - How to deploy arbor
- [YSNP Comparison](./ysnp-comparison.md) - What changed from YSNP
