# Arbor Documentation

Welcome to Arbor's documentation! This directory contains comprehensive design documentation for the Arbor authorization system.

## Purpose

These documents serve as:
1. **Design specifications** for implementation
2. **Architectural decisions** and rationale
3. **Reference documentation** for future development
4. **Context for AI assistants** working on the codebase

## Getting Started

If you're new to Arbor, read in this order:

1. **[Architecture](./architecture.md)** - Start here! Overview of the entire system
2. **[Data Model](./data-model.md)** - Core types and structures
3. **[Authorization Flow](./authorization-flow.md)** - How authorization decisions work
4. **[Implementation Roadmap](./implementation-roadmap.md)** - What needs to be built

## Core Documentation

### System Design

- **[Architecture](./architecture.md)** - High-level system design, components, data flow
  - Service architecture (indexer + authorizer)
  - Performance optimizations
  - Deployment models
  - Design philosophy

- **[Pub/Sub Protocol](./pub-sub-protocol.md)** - Delta distribution protocol (indexer → authorizer)
  - Message types and flow
  - Error handling and retries
  - Serialization and compression
  - Monitoring and configuration

- **[Client Libraries](./client-libraries.md)** - Multi-language client SDKs
  - Dual transport support (Unix Socket + gRPC)
  - Auto-detection and configuration
  - Node.js, Rust, Python implementations
  - Performance optimization techniques

- **[arbor-pg](./arbor-pg.md)** - PostgreSQL native extension (potential feature)
  - Authorization as SQL functions via pgrx
  - Row Level Security integration
  - pgvector / RAG query patterns
  - Performance characteristics and scaling limits

- **[Filter Generation](./filter-generation.md)** - Policy-to-filter predicate pushdown (potential feature)
  - Condition AST → database-native filter IR
  - Partial evaluation (principal known, resource unknown)
  - SQL, MongoDB, Elasticsearch translators
  - InHierarchy resolution via precomputed closure

- **[Policy Validation](./policy-validation.md)** - Three-stage validation strategy
  - Write time: Reject invalid policies
  - Index time: Skip with alerts (availability)
  - Eval time: Fail closed (security)
  - Error handling and monitoring

### Data and Types

- **[Data Model](./data-model.md)** - Core data structures
  - Entity (principals, resources, hierarchies)
  - Policy (permit/forbid, targets, conditions)
  - Action (type-scoped and global)
  - ActionSet (role-like groups)
  - Attributes (nested key-value data)
  - Conditions (boolean expressions)

- **[Snapshot Format](./snapshot-format.md)** - Index structure
  - Snapshot generation process
  - Delta format and operations
  - Transitive closure computation
  - Checksum verification

### Authorization Engine

- **[Authorization Flow](./authorization-flow.md)** - How decisions are made
  - check() operation (permit/deny decisions)
  - list_resources() (what can be accessed)
  - list_principals() (who can access)
  - list_actions() (what actions are allowed)
  - Policy evaluation semantics
  - Performance optimizations

- **[Bytecode VM](./bytecode-vm.md)** - Condition evaluation engine
  - OpCode instruction set
  - Stack-based interpreter
  - Compilation strategy
  - Variable resolution
  - Performance characteristics

### Implementation

- **[Implementation Roadmap](./implementation-roadmap.md)** - What to build next
  - V1: Minimum viable product (8-12 weeks)
  - V2: Production hardening (8-12 weeks)
  - V3: Scale and optimization (future)
  - Milestones and success metrics
  - Risk mitigation

## Document Status

| Document | Status | Completeness |
|----------|--------|--------------|
| architecture.md | ✅ Final | 100% |
| data-model.md | ✅ Final | 100% |
| authorization-flow.md | ✅ Final | 100% |
| bytecode-vm.md | ✅ Final | 100% |
| snapshot-format.md | ✅ Final | 100% |
| pub-sub-protocol.md | ✅ Final | 100% |
| client-libraries.md | ✅ Final | 100% |
| policy-validation.md | ✅ Final | 100% |
| implementation-roadmap.md | ✅ Final | 100% |
| ysnp-comparison.md | ✅ Final | 100% |
| arbor-pg.md | 💡 Potential | 100% |
| filter-generation.md | 💡 Potential | 100% |

## Design Decisions

### Key Architectural Choices

1. **Separate Indexer and Authorizer Services**
   - **Why**: Horizontal scalability, separation of concerns
   - **Trade-off**: Network overhead vs. scalability
   - **Document**: [Architecture](./architecture.md)

2. **Bytecode VM for Condition Evaluation**
   - **Why**: 2-4x faster than AST traversal, enables future JIT
   - **Trade-off**: Compilation complexity vs. performance
   - **Document**: [Bytecode VM](./bytecode-vm.md)

3. **Pub/Sub Delta Distribution**
   - **Why**: Efficient updates, supports many authorizers
   - **Trade-off**: Complexity vs. efficiency
   - **Document**: [Pub/Sub Protocol](./pub-sub-protocol.md)

4. **In-Memory Snapshots (V1)**
   - **Why**: Simplicity, defer persistence to V2
   - **Trade-off**: Cold start time vs. complexity
   - **Document**: [Architecture](./architecture.md), [Implementation Roadmap](./implementation-roadmap.md)

5. **Roaring Bitmaps for Indexes**
   - **Why**: Compressed, fast set operations
   - **Trade-off**: None (pure win)
   - **Document**: [Snapshot Format](./snapshot-format.md)

6. **Hybrid Action Model**
   - **Why**: Flexibility (type-scoped + global actions)
   - **Trade-off**: Slight complexity vs. flexibility
   - **Document**: [Data Model](./data-model.md)

7. **Dual Transport (UDS + gRPC)**
   - **Why**: <100μs sidecar latency + network flexibility
   - **Trade-off**: Complexity vs. performance + flexibility
   - **Document**: [Client Libraries](./client-libraries.md)

8. **Unified Client Libraries**
   - **Why**: Same API for both transports, auto-detection
   - **Trade-off**: None (pure UX win)
   - **Document**: [Client Libraries](./client-libraries.md)

9. **Forbid Takes Precedence**
   - **Why**: Security - explicit denies can't be overridden
   - **Trade-off**: None (security requirement)
   - **Document**: [Authorization Flow](./authorization-flow.md)

## Implementation Status

### ✅ Complete (Foundation)

- Core types (Entity, Policy, Action, Attributes, Condition)
- Graph storage with validation
- Index snapshot structure
- OpCode definitions

### 🚧 In Progress (V1)

- Bytecode VM implementation
- Snapshot builder
- Authorization operations (check, list)
- Services (indexer, authorizer)

### 📋 Planned (V2+)

- External persistence (S3)
- HA indexers
- Advanced features (InNetwork, temporal policies)

See [Implementation Roadmap](./implementation-roadmap.md) for details.

## Development Workflow

### 1. Understanding the System

```
Read: Architecture → Data Model → Authorization Flow
```

### 2. Implementing a Feature

```
1. Check Implementation Roadmap for priority
2. Read relevant design doc
3. Implement with tests
4. Update roadmap status
```

### 3. Adding New Features

```
1. Design feature (update or create doc)
2. Get review/approval
3. Implement
4. Update docs if behavior changes
```

## Contributing

When working on Arbor:

1. **Read the relevant docs first** - Don't guess at design intent
2. **Update docs when making changes** - Keep docs in sync with code
3. **Ask questions** - If docs are unclear, ask and improve them
4. **Test thoroughly** - Correctness > Speed (but both matter)

## Questions or Clarifications?

If any documentation is unclear or incomplete, please:
1. Open an issue describing the confusion
2. Suggest improvements or ask questions
3. We'll update the docs to clarify

## Document Conventions

### Status Markers

- ✅ **Decided**: Final decision, implement as specified
- 🚧 **V1 Scope**: Planned for version 1
- 🔮 **Future**: Planned for later versions
- ❓ **TBD**: Still needs discussion/decision

### Priority Markers

- 🔴 **Critical**: Required for V1, blocks other work
- 🟡 **Important**: Needed for V1, but not blocking
- 🟢 **Nice-to-Have**: Improves experience, not required for V1

### Code Examples

All code examples are in Rust and represent **design intent**, not necessarily final API. Implementations may differ slightly for practical reasons, but should maintain the spirit of the design.

## Versioning

This documentation is versioned alongside the code:
- **Current Version**: V1 (in development)
- **Target Version**: Specified in each document
- **Last Updated**: 2026-02-25

## Related Resources

### External Documentation

- **Roaring Bitmaps**: https://roaringbitmap.org/
- **OpenTelemetry**: https://opentelemetry.io/
- **gRPC**: https://grpc.io/

### Comparison with Other Systems

- **Cedar (AWS)**: Policy language focus, no list operations
- **OpenFGA (Auth0)**: Slow with large hierarchies (Postgres recursion)
- **Ory Keto**: Zanzibar-based, different model
- **Arbor**: Graph-based, fast hierarchies, bytecode evaluation

See [Architecture](./architecture.md#comparison-with-alternatives) for details.

## Contact

For questions about Arbor's design or implementation, please open an issue in the repository.

---

**Last Updated**: 2026-02-25
**Status**: Design Complete, Implementation In Progress
**Next Milestone**: V1 MVP (8-12 weeks)
