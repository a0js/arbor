# Filter Generation (Potential Feature)

> **Status**: Potential future feature. Not planned for V1 or V2. Requires the core authorization engine and client library to be complete first.

Filter generation allows Arbor to translate authorization policies into database-native filter predicates. Instead of returning a list of permitted resource IDs, Arbor returns a `FilterNode` IR (intermediate representation) that can be pushed directly into any data store as a `WHERE` clause — similar to OPA's partial evaluation / compile API.

---

## Problem

`list_resources(principal, action)` has two scaling limits with the current ID-set approach:

1. **Large authorized sets**: Returning 50K UUIDs and passing them as `WHERE id IN (...)` degrades DB performance above ~10K IDs.
2. **DB round-trips**: The engine must compute the full authorized set before the DB query can begin. For vector search (pgvector, Pinecone), this serializes two operations that could be combined.

Filter generation solves both: Arbor returns a filter predicate, and the DB evaluates it natively during its own query execution.

---

## How It Works

### Compile-time: AST → FilterNode

At index time, the condition compiler runs two passes over each `Condition` AST:

```
Condition AST
    ├── → CompiledCondition (bytecode, for VM evaluation)
    └── → Option<FilterNode>  (filter IR, for predicate pushdown)
```

`FilterNode` is stored in the snapshot alongside the bytecode. Not all conditions are translatable — those that aren't produce `None` and fall back to the existing two-phase bitmap + VM evaluation.

### Query time: Partial evaluation

When `list_filters(principal, action, entity_type)` is called, the principal is fully known. Any condition referencing principal attributes resolves to a constant immediately:

```
Policy condition:   principal.department == resource.department
Principal dept:     "engineering"
Resolved filter:    WHERE resource_department = 'engineering'

Policy condition:   principal.clearance >= resource.sensitivity
Principal clearance: 3
Resolved filter:    WHERE sensitivity <= 3
```

Conditions that reference only the principal collapse entirely:
- If the principal satisfies the condition → the policy applies unconditionally → `Always(true)` filter
- If the principal does not → the policy never applies → `Always(false)` → pruned from output

### Result shape

```rust
pub struct ListFiltersResult {
    /// OR of all applicable permit policy filters.
    pub permit_filter: FilterNode,
    /// OR of all applicable forbid policy filters.
    pub forbid_filter: FilterNode,
}
// Caller applies: WHERE (permit_filter) AND NOT (forbid_filter)
```

Permit is the union of all matching permit policies. A resource is excluded if any forbid policy matches — consistent with Arbor's existing forbid-takes-precedence semantics.

---

## FilterNode IR

```rust
pub enum FilterNode {
    And(Vec<FilterNode>),
    Or(Vec<FilterNode>),
    Not(Box<FilterNode>),
    Comparison { field: FilterField, op: FilterOp, value: FilterValue },
    InSet      { field: FilterField, values: Vec<FilterValue> },
    /// InHierarchy: resource must be a descendant of the given anchor.
    /// Resolved at query time using Arbor's precomputed transitive closure.
    /// Expands to: WHERE resource_id IN (all_descendants_of_anchor).
    InHierarchy { field: FilterField, descendant_ids: Vec<Uuid> },
    InNetwork   { field: FilterField, network: IpNetwork },
    /// Constant after partial evaluation — pruned by translators.
    Always(bool),
}

pub enum FilterField {
    /// resource.sensitivity_level — the remaining predicate after principal attrs resolved
    ResourceAttr(AttributePath),
    /// Retained only when the calling context provides it (e.g., context.ip_address)
    ContextAttr(AttributePath),
}

pub enum FilterOp {
    Eq, Ne, Lt, Le, Gt, Ge,
    StartsWith, EndsWith, Contains, Like,
}

pub enum FilterValue {
    String(String),
    Int(i64),
    Float(f64),
    Bool(bool),
    Uuid(Uuid),
}
```

---

## Condition Translation Reference

| Arbor condition | FilterNode | SQL output |
|---|---|---|
| `resource.dept == "eng"` | `Comparison(Eq, "eng")` | `WHERE dept = 'eng'` |
| `resource.size < 1000` | `Comparison(Lt, 1000)` | `WHERE size < 1000` |
| `resource.name StartsWith "doc"` | `Comparison(StartsWith, "doc")` | `WHERE name LIKE 'doc%'` |
| `resource.tag Like "re*rt"` | `Comparison(Like, "re*rt")` | `WHERE tag LIKE 're%rt'` |
| `InHierarchy(resource, folder:X)` | `InHierarchy { descendant_ids: [...] }` | `WHERE id IN (uuid1, uuid2, ...)` |
| `IsType(resource, "document")` | `Comparison(Eq, "document")` on type field | `WHERE entity_type = 'document'` |
| `InNetwork(context.ip, 10.0.0.0/8)` | `InNetwork { network: 10.0.0.0/8 }` | `WHERE ip <<= '10.0.0.0/8'` (pg) |
| `AND(cond_a, cond_b)` | `And([node_a, node_b])` | `WHERE (a) AND (b)` |
| `OR(cond_a, cond_b)` | `Or([node_a, node_b])` | `WHERE (a) OR (b)` |
| `NOT(cond)` | `Not(node)` | `WHERE NOT (...)` |
| `principal.dept == "eng"` (principal known) | `Always(true/false)` | pruned |

### InHierarchy resolution

`InHierarchy(resource, folder:X)` in the VM evaluates as:

```rust
resource.ancestors.contains(folder_X_idx)  // true if resource is a descendant of folder:X
```

At filter generation time, the equivalent predicate is: "resource is one of the precomputed descendants of folder:X." Arbor's snapshot already has the full transitive descendant set as a `RoaringBitmap`. The filter resolver expands it to UUIDs:

```rust
// At query time, when generating the filter:
let descendant_ids = snapshot
    .descendants_of(anchor_idx)       // precomputed RoaringBitmap
    .iter()
    .filter_map(|idx| snapshot.index_to_uuid[idx as usize])
    .collect::<Vec<Uuid>>();

FilterNode::InHierarchy { field: FilterField::ResourceAttr("id".into()), descendant_ids }
```

SQL output:
```sql
WHERE id IN ('uuid-1', 'uuid-2', 'uuid-3', ...)
```

The DB does not need to understand hierarchy at all. Arbor's precomputed closure handles the tree traversal entirely at filter generation time. For very large hierarchies (>10K descendants), this transitions from a filter into a large IN list — the same scaling limit that applies to `list_resources()` ID sets. In practice, hierarchical policies typically target organizational structures (departments, folders, teams) where descendant counts are in the hundreds to low thousands.

### What cannot be translated

- Conditions referencing only context attributes where context is unknown at filter generation time (e.g., `context.ip_address` when no context is provided) — fall back to VM evaluation
- Custom evaluation hooks (if added in a future version)

Untranslatable conditions produce `FilterNode = None` for that policy. Those policies fall back to the existing two-phase bitmap + VM approach.

---

## Proposed Crate: `arbor-filters`

```
crates/arbor-filters/
  src/
    filter_ir.rs         // FilterNode, FilterField, FilterOp, FilterValue types
    generator.rs         // Condition AST → FilterNode  (runs at index time)
    partial_eval.rs      // FilterNode + known attrs → simplified FilterNode
    translators/
      sql.rs             // FilterNode → (sql_fragment: String, params: Vec<FilterValue>)
      mongo.rs           // FilterNode → serde_json::Value  (MongoDB filter document)
      elasticsearch.rs   // FilterNode → serde_json::Value  (ES query DSL)
```

Callers pick a translator and receive a ready-to-use filter:

```rust
use arbor_filters::translators::sql::SqlTranslator;

let result = engine.list_filters(principal_idx, action_idx, entity_type)?;
let (where_clause, params) = SqlTranslator::translate(&result.permit_filter)?;
// → ("(dept = $1 OR sensitivity <= $2)", [Value::String("eng"), Value::Int(3)])

let query = format!(
    "SELECT id, embedding <-> $embed AS distance \
     FROM documents \
     WHERE ({where_clause}) AND NOT ({forbid_clause}) \
     ORDER BY distance LIMIT 10"
);
```

---

## Comparison with OPA's Compile API

| | OPA compile API | Arbor filter generation |
|---|---|---|
| Output format | Structured AST (Rego partial) | `FilterNode` IR |
| First-party translators | Community tools only | SQL, MongoDB, Elasticsearch built-in |
| Hierarchy support | Manual (policy author must model) | Native via precomputed closure expansion |
| Partial evaluation | Yes (unknown = variable) | Yes (principal known, resource unknown) |
| Condition language | Turing-complete Rego | Subset — not all conditions translatable |
| Forbid / deny | Not native (modeled as exclusion) | First-class — `forbid_filter` in result |

The key advantage over OPA: Arbor's precomputed transitive closures mean `InHierarchy` conditions produce a flat ID list at filter generation time rather than requiring the DB to perform recursive CTE traversal. Hierarchy is resolved by Arbor, not delegated to the database.

---

## Integration with arbor-pg

When combined with the [arbor-pg PostgreSQL extension](./arbor-pg.md), filter generation enables a native SQL function that returns a WHERE clause fragment:

```sql
-- Returns a SQL fragment and bind parameters for the pgvector query
SELECT arbor_permitted_filter(
  current_setting('app.user_id')::uuid,
  'read',
  'document'
);
-- → "dept = $1 AND sensitivity <= $2"  with params ['engineering', 3]
```

This replaces the `arbor_permitted_ids()` ID-list approach for datasets where the authorized set would be large, eliminating the `WHERE id IN (50000 values)` scaling wall entirely.

---

## Prerequisites

- `arbor-types`: `Condition` AST (already exists)
- `arbor-bytecode`: condition compiler (complete — filter generator runs alongside)
- `arbor-index-snapshot`: `Snapshot` must store `Option<FilterNode>` alongside `CompiledCondition` in `IndexedPolicy`
- `arbor-client`: client library (prerequisite for integration with application code)
- `arbor-pg` (optional): for native PostgreSQL function integration

---

## Related Documentation

- [Authorization Flow](./authorization-flow.md) — `list_resources()` operation
- [Bytecode VM](./bytecode-vm.md) — condition compilation pipeline
- [arbor-pg](./arbor-pg.md) — PostgreSQL extension (natural integration target)
- [Implementation Roadmap](./implementation-roadmap.md) — delivery sequence
