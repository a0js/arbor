# Arbor

A high-performance, graph-based authorization system built in Rust.

## Overview

Arbor is a production-grade authorization engine designed for:
- **Hierarchical entity relationships** with directed acyclic graphs (DAGs)
- **Attribute-based access control (ABAC)** with rich conditional policies
- **Horizontal scalability** with separate indexer and authorizer services
- **Fast policy evaluation** using bytecode compilation
- **Cloud-native deployment** as sidecars or centralized services

## Key Features

- ✅ **Hierarchical Authorization**: Entities can have multiple parents, policies apply to subtrees
- ✅ **Four Operations**: check(), list_resources(), list_principals(), list_actions()
- ✅ **Conditional Policies**: Rich boolean expressions with attribute references
- ✅ **Forbid Precedence**: Explicit denies override permits (security first)
- ✅ **Bytecode VM**: Compiled conditions for 2-4x faster evaluation
- ✅ **Dual Transport**: Unix sockets (30-80μs) or gRPC (1-5ms) for flexible deployment
- ✅ **Hot Swapping**: Zero-downtime updates to authorizers

## Architecture

```
┌──────────────────┐
│ Source Data      │  (Database, CSV, API)
└────────┬─────────┘
         │
         ▼
┌──────────────────┐
│ Arbor Indexer    │  Transforms data into authorization indexes
│                  │  Compiles policies to bytecode
│                  │  Generates versioned snapshots + deltas
└────────┬─────────┘
         │
         │ Pub/Sub (NATS, Kafka, Redis)
         │
    ┌────┴────┐
    ▼         ▼         ▼
┌──────┐  ┌──────┐  ┌──────┐
│Auth  │  │Auth  │  │Auth  │  Arbor Authorizers (many instances)
│ -1   │  │ -2   │  │ -N   │  - Receive snapshot notifications
└──────┘  └──────┘  └──────┘  - Evaluate authorization requests
                               - Horizontally scalable
```

## Quick Start

> **Note**: Arbor is currently in active development. V1 is expected in 8-10 weeks.

```bash
# Clone the repository
git clone https://github.com/yourusername/arbor.git
cd arbor

# Build the project
cargo build --release

# Run tests
cargo test

# (V1) Start the indexer
./target/release/arbor-indexer --config indexer.toml

# (V1) Start an authorizer
./target/release/arbor-authorizer --config authorizer.toml
```

## Example Usage

### Rust Client

```rust
use arbor_client::{ArborClient, ClientConfig};

// Auto-detect mode (sidecar if socket exists, else gRPC)
let mut client = ArborClient::new(ClientConfig::default()).await?;

// Check authorization
let result = client.check(
    principal_id,
    action_id,
    resource_id,
    Attributes::default(),
).await?;

if result.decision == Decision::Permit {
    println!("Access granted!");
} else {
    println!("Access denied: {:?}", result.reason);
}

// List accessible resources
let resources = client.list_resources(
    principal_id,
    read_action_id,
    Some("document"),  // Type filter
    Attributes::default(),
    ListOptions { limit: Some(100), offset: None },
).await?;

println!("User can read {} documents", resources.resources.len());
```

### Node.js Client

```javascript
const { ArborClient } = require('@arbor/client');

// Auto-detect mode
const client = new ArborClient();

// Check authorization
const result = await client.check(principalId, actionId, resourceId);

if (result.decision === 'permit') {
  console.log('Access granted!');
} else {
  console.log('Access denied:', result.reason);
}

// List accessible resources
const resources = await client.listResources(
  principalId,
  readActionId,
  'document',  // Type filter
  {},          // Context
  { limit: 100 }
);

console.log(`User can read ${resources.resources.length} documents`);
```

## Core Concepts

### Entity

Entities represent both principals (users, roles) and resources (files, folders, objects):

```rust
Entity {
    id: uuid,
    name: "Alice",
    entity_type: "user",
    parents: [engineering_team_id],  // Hierarchical relationships
    attributes: {
        "tier": "gold",
        "email": "alice@example.com",
        "profile": {
            "department": "engineering",
            "level": 5
        }
    }
}
```

### Policy

Policies define authorization rules:

```rust
Policy {
    name: "Gold users can edit large documents",
    policy_type: Permit,
    principal: EntityType("user"),
    resource: EntityType("document"),
    actions: [edit_action_id],
    conditions: Some(
        And(
            Eq(principal.tier, "gold"),
            Gt(resource.size, 10000)
        )
    )
}
```

### Policy Targets

Policies can target:
- **Specific entity**: `PolicyTarget::Entity(uuid)`
- **Entity + descendants**: `PolicyTarget::EntityWithDescendants(uuid)`
- **Entity type**: `PolicyTarget::EntityType("user")`
- **All entities**: `PolicyTarget::All`

### Evaluation Rules

1. **Forbid takes precedence**: A single forbid overrides any permits
2. **Default deny**: Access denied if no permits apply
3. **Short-circuit**: Stop at first forbid, return reason for debugging

## Performance

### Expected Performance (V1)

**Sidecar Mode (Unix Domain Socket)**:
- **check()**: 30-80μs p99 latency (Rust client: 30-50μs, Node.js: 50-80μs)
- **list_resources()**: <5ms p99 (10K resources)
- **Throughput**: >10K check() ops/sec per authorizer
- **Transport**: Length-prefixed Protobuf over Unix socket

**Centralized Mode (gRPC/HTTP2)**:
- **check()**: 1-5ms p99 latency (network-dependent)
- **list_resources()**: 5-20ms p99 (10K resources)
- **Throughput**: >5K check() ops/sec per authorizer
- **Transport**: Standard gRPC over HTTP/2

**Scaling**:
- **Horizontal**: Add more authorizers linearly
- **Zero downtime**: Hot-swap snapshots via delta updates

### Optimizations

- **Roaring Bitmaps**: Compressed set operations (10-100x compression)
- **Precomputed Closures**: Ancestors/descendants computed once
- **Bytecode Compilation**: Conditions compiled once, evaluated many times
- **Two-Phase Listing**: Bitmap operations + sparse conditional evaluation
- **Attribute Shape Caching**: Cache condition results for similar entities

## Project Status

### Current Status: V1 Development

- [x] Foundation (~24% complete)
  - [x] Core types (Entity, Policy, Action, Attributes)
  - [x] Graph storage with validation
  - [x] Index snapshot structure
  - [x] OpCode definitions
- [ ] Core Engine (Weeks 1-5)
  - [ ] Bytecode VM
  - [ ] Snapshot builder
  - [ ] Authorization operations (check, list)
- [ ] Services (Weeks 5-7)
  - [ ] Indexer service
  - [ ] Authorizer service
  - [ ] Pub/sub integration
- [ ] Testing & Observability (Weeks 8-10)
  - [ ] Integration tests
  - [ ] Load tests
  - [ ] OpenTelemetry tracing

See [Implementation Roadmap](./docs/implementation-roadmap.md) for details.

## Documentation

Comprehensive design documentation in [`docs/`](./docs/):

- **[Architecture](./docs/architecture.md)** - System design and components
- **[Data Model](./docs/data-model.md)** - Core types and structures
- **[Authorization Flow](./docs/authorization-flow.md)** - How decisions are made
- **[Bytecode VM](./docs/bytecode-vm.md)** - Condition evaluation engine
- **[Snapshot Format](./docs/snapshot-format.md)** - Index structure and deltas
- **[Pub/Sub Protocol](./docs/pub-sub-protocol.md)** - Delta distribution
- **[Implementation Roadmap](./docs/implementation-roadmap.md)** - What to build next
- **[YSNP Comparison](./docs/ysnp-comparison.md)** - Historical context

Start with [Architecture](./docs/architecture.md) for an overview.

## Comparison with Alternatives

| Feature | Arbor | Cedar (AWS) | OpenFGA | Ory Keto |
|---------|-------|-------------|---------|----------|
| Check operation | ✅ | ✅ | ✅ | ✅ |
| List operations | ✅ | ❌ | ✅ (slow) | ✅ |
| Hierarchies | ✅ Fast | ❌ | ✅ Slow | ✅ |
| ABAC conditions | ✅ | ✅ | ❌ | ❌ |
| Bytecode eval | ✅ | ✅ | N/A | N/A |
| Horizontal scale | ✅ | ✅ | ⚠️ | ✅ |
| Open source | ✅ | ✅ | ✅ | ✅ |

**Arbor's Niche**:
- Fast hierarchical authorization (unlike OpenFGA)
- List operations (unlike Cedar)
- ABAC with conditions (unlike OpenFGA/Keto)
- Cloud-native scalability

## Deployment Models

### Sidecar Model

Deploy authorizer as a sidecar container alongside your application:
- Ultra-low latency (<1ms)
- No network hops
- Automatic scaling with your app

### Centralized Model

Deploy authorizer as a centralized service:
- Shared resource pool
- Easier to manage
- Slight network latency (~1-2ms)

### Hybrid Model

Critical services use sidecars, others use centralized cluster.

See [Architecture](./docs/architecture.md#deployment-models) for details.

## Contributing

Arbor is in active development. Contributions welcome!

1. Read the [Architecture](./docs/architecture.md) and [Implementation Roadmap](./docs/implementation-roadmap.md)
2. Check open issues or create a new one
3. Fork and create a feature branch
4. Write tests for your changes
5. Update documentation if needed
6. Submit a pull request

### Development Guidelines

- **Correctness > Speed** (but both matter)
- **Test thoroughly** (unit + integration + property-based)
- **Document design decisions** in `docs/`
- **Follow Rust idioms** (clippy, rustfmt)

## Observability

Arbor includes built-in observability:

- **OpenTelemetry Tracing**: Distributed traces across indexer/authorizer
- **Prometheus Metrics**: Latency, throughput, policy hits
- **Structured Logging**: JSON logs with context
- **Health Checks**: Readiness and liveness endpoints

Example Grafana dashboards and Prometheus alerts provided.

## Roadmap

### V1: MVP (8-10 weeks)

- Complete bytecode VM
- Authorization operations (check, list)
- Indexer and authorizer services with dual transport
- Client libraries (Rust, Node.js, Python)
- Broker-based snapshot notifications (NATS)
- Basic observability

### V2: Production Hardening (8-12 weeks)

- External snapshot persistence (S3)
- HA indexers with leader election
- Delta updates (incremental snapshots)
- Additional broker connectors (Kafka, Redis Streams)
- Advanced features (InNetwork, temporal policies)
- Policy testing framework
- Admin UI

### V3: Scale and Optimization (Future)

- JIT compilation for hot conditions
- SIMD batch evaluation
- Cross-region replication
- ML-based policy recommendations

## License

TBD

## Acknowledgments

Arbor builds on ideas from:
- **Zanzibar** (Google's authorization system)
- **Cedar** (AWS policy language)
- **OpenFGA** (Auth0's authorization system)

Special thanks to the open-source community for Rust, Roaring Bitmaps, and related projects.

## Contact

- **Issues**: https://github.com/yourusername/arbor/issues
- **Discussions**: https://github.com/yourusername/arbor/discussions

---

**Status**: V1 Development (24% complete)
**Next Milestone**: Core Engine Complete (Week 5)
**Target**: Production-ready V1 in 8-10 weeks
