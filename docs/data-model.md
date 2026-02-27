# Data Model

This document describes Arbor's core data types and their relationships.

## Overview

Arbor's data model consists of five primary types:

1. **Entity**: Principals, resources, or both (users, files, folders, organizations)
2. **Policy**: Authorization rules (who can do what on which resources)
3. **Action**: Operations that can be performed (read, write, delete, edit)
4. **ActionSet**: Groups of actions (like roles - "editor", "viewer")
5. **Attributes**: Nested key-value data attached to entities

## Entity

Entities represent both **principals** (who) and **resources** (what). An entity can be either, or both.

```rust
pub struct Entity {
    /// Unique identifier
    pub id: Uuid,

    /// Human-readable name
    pub name: String,

    /// Entity type (user, file, folder, organization, etc.)
    pub entity_type: EntityTypeId,

    /// Parent entities (for hierarchies)
    pub parents: Vec<Uuid>,

    /// Arbitrary attributes
    pub attributes: Attributes,
}
```

### Entity Types

Entity types are string-based identifiers that categorize entities:

```rust
pub struct EntityTypeId(StringId<EntityTypeMarker>);

// Examples:
let user_type = EntityTypeId::new("user");
let file_type = EntityTypeId::new("file");
let folder_type = EntityTypeId::new("folder");
let organization_type = EntityTypeId::new("organization");
```

**Type-Safe IDs**: `EntityTypeId` uses phantom types to prevent mixing with other ID types at compile time.

### Hierarchies

Entities can have **multiple parents**, forming a **directed acyclic graph (DAG)**:

```
Organization
    ├── Team A
    │   ├── Alice
    │   └── Bob
    └── Team B
        ├── Bob      (Bob is in both teams)
        └── Carol
```

**Circular Dependency Prevention**: The graph validates that adding parents doesn't create cycles.

**Transitive Closure**: When a snapshot is generated, **all ancestors and descendants** are precomputed for each entity:

```rust
pub struct IndexedEntity {
    pub entity: Entity,
    pub index: u32,

    /// All ancestors (direct + indirect)
    pub ancestors: RoaringBitmap,

    /// All descendants (direct + indirect)
    pub descendants: RoaringBitmap,

    // ... other fields
}
```

**Example**:
```
If Alice → Team A → Organization:
- Alice.ancestors = {Team A, Organization}
- Alice.descendants = {}
- Team A.ancestors = {Organization}
- Team A.descendants = {Alice}
- Organization.ancestors = {}
- Organization.descendants = {Team A, Alice}
```

### Attributes

Entities have attributes for storing additional data:

```rust
pub type Attributes = BTreeMap<AttributeNameId, AttributeValue>;

pub enum AttributeValue {
    Scalar(ScalarValue),
    EntityRef(Uuid),
    Set(Vec<AttributeValue>),
    Object(Attributes),  // Nested attributes
}

pub enum ScalarValue {
    String(String),
    Integer(i64),
    Float(OrderedFloat<f64>),
    Bool(bool),
    Timestamp(i64),  // Unix timestamp in milliseconds
}
```

**Example**:
```json
{
  "id": "550e8400-e29b-41d4-a716-446655440000",
  "name": "Alice",
  "entity_type": "user",
  "parents": [],
  "attributes": {
    "email": "alice@example.com",
    "tier": "gold",
    "profile": {
      "age": 30,
      "department": "engineering",
      "permissions": {
        "canExport": true,
        "maxFileSize": 10485760
      }
    },
    "tags": ["admin", "verified"]
  }
}
```

**Nested Access**: Policy conditions can reference nested attributes:
```rust
// principal.profile.permissions.canExport
VariableRef {
    scope: Scope::Principal,
    path: vec!["profile", "permissions", "canExport"]
}
```

## Policy

Policies define authorization rules.

```rust
pub struct Policy {
    /// Unique identifier
    pub id: Uuid,

    /// Human-readable name
    pub name: String,

    /// Permit or Forbid
    pub policy_type: PolicyType,

    /// Who the policy applies to
    pub principal: PolicyTarget,

    /// What the policy applies to
    pub resource: PolicyTarget,

    /// Which actions are allowed/forbidden
    pub actions: Vec<Uuid>,

    /// Action sets (expanded to actions)
    pub action_sets: Vec<Uuid>,

    /// Optional condition
    pub conditions: Option<Condition>,

    /// Precomputed dependencies (for optimization)
    pub dependencies: Vec<VariableRef>,
}

pub enum PolicyType {
    Permit,  // Allow access
    Forbid,  // Deny access (takes precedence)
}
```

### Policy Targets

Policies can target entities in four ways:

```rust
pub enum PolicyTarget {
    /// Specific entity by UUID
    Entity(Uuid),

    /// Entity and all its descendants
    EntityWithDescendants(Uuid),

    /// All entities of a specific type
    EntityType(EntityTypeId),

    /// All entities
    All,
}
```

**Examples**:

1. **Specific Entity**:
   ```rust
   Policy {
       principal: PolicyTarget::Entity(alice_id),
       resource: PolicyTarget::Entity(file_id),
       // Alice can access this specific file
   }
   ```

2. **Descendants**:
   ```rust
   Policy {
       principal: PolicyTarget::EntityWithDescendants(team_a_id),
       resource: PolicyTarget::EntityWithDescendants(shared_folder_id),
       // All members of Team A can access all files in shared folder
   }
   ```

3. **Entity Type**:
   ```rust
   Policy {
       principal: PolicyTarget::EntityType("user"),
       resource: PolicyTarget::EntityType("public_file"),
       // All users can access all public files
   }
   ```

4. **All**:
   ```rust
   Policy {
       principal: PolicyTarget::All,
       resource: PolicyTarget::Entity(public_doc_id),
       // Everyone can access this public document
   }
   ```

### Conditions

Policies can have optional conditions that must evaluate to true:

```rust
Policy {
    name: "Gold tier users can edit large files".to_string(),
    policy_type: PolicyType::Permit,
    principal: PolicyTarget::EntityType("user"),
    resource: PolicyTarget::EntityType("file"),
    actions: vec![edit_action_id],
    conditions: Some(Condition::And(
        Box::new(Condition::Eq(
            ValueExpr::Variable(VariableRef {
                scope: Scope::Principal,
                path: vec!["tier"]
            }),
            ValueExpr::Literal(ScalarValue::String("gold".into()))
        )),
        Box::new(Condition::Gt(
            ValueExpr::Variable(VariableRef {
                scope: Scope::Resource,
                path: vec!["size"]
            }),
            ValueExpr::Literal(ScalarValue::Integer(10_000_000))
        ))
    )),
    // ...
}
```

See [Conditions](#conditions-1) section below for details.

## Action

Actions represent operations that can be performed.

```rust
pub struct Action {
    /// Unique identifier (deterministic v5 UUID)
    pub id: Uuid,

    /// Action name (e.g., "read", "write", "delete")
    pub name: String,

    /// Optional entity type this action applies to
    pub entity_type: Option<EntityTypeId>,

    /// Description
    pub description: Option<String>,
}
```

### Hybrid Action Model

Arbor supports both **type-scoped** and **global** actions:

**Type-Scoped Actions**:
```rust
Action {
    id: uuid_v5("file", "edit"),
    name: "edit".to_string(),
    entity_type: Some("file"),
}

Action {
    id: uuid_v5("profile", "edit"),
    name: "edit".to_string(),
    entity_type: Some("profile"),
}
```
These are **different actions** because they apply to different entity types.

**Global Actions**:
```rust
Action {
    id: uuid_v5("audit"),
    name: "audit".to_string(),
    entity_type: None,  // Applies to all entity types
}
```

### Deterministic UUIDs

Action UUIDs are **deterministically generated** from name + entity type:

```rust
fn generate_action_id(name: &str, entity_type: Option<&str>) -> Uuid {
    let namespace = Uuid::NAMESPACE_DNS;
    let input = match entity_type {
        Some(et) => format!("{}:{}", et, name),
        None => name.to_string(),
    };
    Uuid::new_v5(&namespace, input.as_bytes())
}
```

**Benefit**: Multiple indexers independently generate the same UUIDs for the same actions.

## ActionSet

Action sets are collections of actions, similar to roles:

```rust
pub struct ActionSet {
    /// Unique identifier
    pub id: Uuid,

    /// Human-readable name (e.g., "editor", "viewer", "admin")
    pub name: String,

    /// Actions in this set
    pub actions: Vec<Uuid>,

    /// Optional metadata
    pub metadata: Attributes,
}
```

**Examples**:

```rust
ActionSet {
    id: editor_set_id,
    name: "editor".to_string(),
    actions: vec![read_action_id, write_action_id, delete_action_id],
}

ActionSet {
    id: viewer_set_id,
    name: "viewer".to_string(),
    actions: vec![read_action_id],
}
```

**Usage in Policies**:

```rust
Policy {
    principal: PolicyTarget::EntityType("user"),
    resource: PolicyTarget::EntityType("file"),
    action_sets: vec![editor_set_id],
    // Expands to: actions = [read, write, delete]
}
```

**Expansion**: During snapshot generation, action sets are expanded to individual actions in `IndexedPolicy::expanded_actions`.

## Conditions

Conditions are boolean expressions that can reference entity attributes.

### Condition AST

```rust
pub enum Condition {
    // ===== Logical Operators =====
    And(Box<Condition>, Box<Condition>),
    Or(Box<Condition>, Box<Condition>),
    Not(Box<Condition>),

    // ===== Comparison Operators =====
    Eq(ValueExpr, ValueExpr),
    Neq(ValueExpr, ValueExpr),
    Lt(ValueExpr, ValueExpr),
    Lte(ValueExpr, ValueExpr),
    Gt(ValueExpr, ValueExpr),
    Gte(ValueExpr, ValueExpr),

    // ===== Set Operators =====
    In(ValueExpr, ValueExpr),           // element in set
    Contains(ValueExpr, ValueExpr),      // set contains element
    ContainsAll(ValueExpr, ValueExpr),   // set contains all of subset
    ContainsAny(ValueExpr, ValueExpr),   // set contains any of subset

    // ===== Attribute Operators =====
    HasAttribute(VariableRef),

    // ===== Network Operators (V2) =====
    InNetwork(ValueExpr, ValueExpr),     // IP in CIDR range
}
```

### Value Expressions

```rust
pub enum ValueExpr {
    /// Literal scalar value
    Literal(ScalarValue),

    /// Variable reference (principal.x, resource.y, context.z)
    Variable(VariableRef),

    /// Entity reference
    EntityRef(Uuid),

    /// Set of values
    Set(Vec<AttributeValue>),
}
```

### Variable References

```rust
pub struct VariableRef {
    /// Scope: Principal, Resource, or Context
    pub scope: Scope,

    /// Path to nested attribute
    pub path: Vec<String>,
}

pub enum Scope {
    Principal,  // Attributes of the requesting principal
    Resource,   // Attributes of the resource being accessed
    Context,    // Contextual data (time, IP, custom)
}
```

**Examples**:

```rust
// principal.tier
VariableRef {
    scope: Scope::Principal,
    path: vec!["tier"]
}

// resource.owner.id
VariableRef {
    scope: Scope::Resource,
    path: vec!["owner", "id"]
}

// context.time
VariableRef {
    scope: Scope::Context,
    path: vec!["time"]
}
```

### Condition Examples

**Simple Comparison**:
```rust
// principal.tier == "gold"
Condition::Eq(
    ValueExpr::Variable(VariableRef {
        scope: Scope::Principal,
        path: vec!["tier"]
    }),
    ValueExpr::Literal(ScalarValue::String("gold".into()))
)
```

**Logical Combination**:
```rust
// principal.tier == "gold" AND resource.size > 1000
Condition::And(
    Box::new(Condition::Eq(...)),
    Box::new(Condition::Gt(...))
)
```

**Set Membership**:
```rust
// principal.role in ["admin", "moderator"]
Condition::In(
    ValueExpr::Variable(VariableRef {
        scope: Scope::Principal,
        path: vec!["role"]
    }),
    ValueExpr::Set(vec![
        AttributeValue::Scalar(ScalarValue::String("admin".into())),
        AttributeValue::Scalar(ScalarValue::String("moderator".into()))
    ])
)
```

**Attribute Existence**:
```rust
// has_attribute(resource.confidential)
Condition::HasAttribute(VariableRef {
    scope: Scope::Resource,
    path: vec!["confidential"]
})
```

**Complex Condition**:
```rust
// (principal.tier == "gold" OR principal.isAdmin == true) AND resource.size < 100000
Condition::And(
    Box::new(Condition::Or(
        Box::new(Condition::Eq(
            ValueExpr::Variable(VariableRef {
                scope: Scope::Principal,
                path: vec!["tier"]
            }),
            ValueExpr::Literal(ScalarValue::String("gold".into()))
        )),
        Box::new(Condition::Eq(
            ValueExpr::Variable(VariableRef {
                scope: Scope::Principal,
                path: vec!["isAdmin"]
            }),
            ValueExpr::Literal(ScalarValue::Bool(true))
        ))
    )),
    Box::new(Condition::Lt(
        ValueExpr::Variable(VariableRef {
            scope: Scope::Resource,
            path: vec!["size"]
        }),
        ValueExpr::Literal(ScalarValue::Integer(100_000))
    ))
)
```

### Condition Dependencies

For optimization, conditions precompute which attributes they depend on:

```rust
impl Condition {
    pub fn compute_dependencies(&self) -> Vec<VariableRef> {
        // Returns all variable references in this condition
        // Used to:
        // 1. Only load necessary attributes
        // 2. Cache evaluation results by attribute shape
    }
}
```

## Type-Safe IDs

Arbor uses phantom types to prevent mixing different ID types:

```rust
pub struct StringId<T> {
    inner: u32,
    _phantom: PhantomData<T>,
}

// Type markers
pub struct EntityTypeMarker;
pub struct AttributeNameMarker;

// Type-safe IDs
pub type EntityTypeId = StringId<EntityTypeMarker>;
pub type AttributeNameId = StringId<AttributeNameMarker>;
```

**Compile-Time Safety**:
```rust
let entity_type_id: EntityTypeId = ...;
let attribute_name_id: AttributeNameId = ...;

// This won't compile:
if entity_type_id == attribute_name_id { ... }  // ❌ Type error
```

## Relationships

### Entity Relationships

```
Entity
  ├── parents: Vec<Uuid>             (direct parents)
  └── (in IndexedEntity)
      ├── ancestors: RoaringBitmap   (all ancestors)
      └── descendants: RoaringBitmap (all descendants)
```

### Policy Relationships

```
Policy
  ├── principal: PolicyTarget  (who)
  ├── resource: PolicyTarget   (what)
  ├── actions: Vec<Uuid>       (which actions)
  └── action_sets: Vec<Uuid>   (which action groups)
```

### Action Relationships

```
Action
  └── entity_type: Option<EntityTypeId>  (scoped to type or global)

ActionSet
  └── actions: Vec<Uuid>  (collection of actions)
```

### Indexed Relationships (Snapshot)

```
IndexedEntity
  ├── ancestors: RoaringBitmap            (transitive closure)
  ├── descendants: RoaringBitmap          (transitive closure)
  ├── principal_of_policies: RoaringBitmap (policies where this is principal)
  └── resource_of_policies: RoaringBitmap  (policies where this is resource)

IndexSnapshot
  ├── entity_type_to_indices: Map<EntityTypeId, RoaringBitmap>
  ├── action_to_policies: Map<Uuid, RoaringBitmap>
  └── [specialized bitmaps for query optimization]
```

## Serialization

### Internal Format (V1)

Use `bincode` for efficient binary serialization:

```rust
use bincode::{serialize, deserialize};

let entity_bytes = serialize(&entity)?;
let entity: Entity = deserialize(&entity_bytes)?;
```

**Characteristics**:
- Fast (microseconds for typical entities)
- Compact (minimal overhead)
- Rust-native (no cross-language support)

### External Format (V2+)

Use Protocol Buffers for cross-language compatibility:

```protobuf
message Entity {
  bytes id = 1;  // UUID bytes
  string name = 2;
  string entity_type = 3;
  repeated bytes parents = 4;
  map<string, AttributeValue> attributes = 5;
}

message AttributeValue {
  oneof value {
    ScalarValue scalar = 1;
    bytes entity_ref = 2;
    AttributeSet set = 3;
    AttributeObject object = 4;
  }
}
```

## Validation Rules

### Entity Validation

- **ID**: Must be valid UUID
- **Name**: Non-empty string
- **Entity Type**: Must exist in type registry
- **Parents**: Must not create circular dependencies
- **Attributes**: Keys must be valid attribute names

### Policy Validation

- **ID**: Must be valid UUID
- **Name**: Non-empty string
- **Principal/Resource Targets**: If Entity or EntityWithDescendants, UUID must exist
- **Actions**: All action UUIDs must exist
- **Action Sets**: All action set UUIDs must exist
- **Conditions**: Must be well-formed (no type errors)

### Action Validation

- **ID**: Must be valid UUID (preferably deterministic v5)
- **Name**: Non-empty string
- **Entity Type**: If specified, must exist in type registry

### ActionSet Validation

- **ID**: Must be valid UUID
- **Name**: Non-empty string
- **Actions**: All action UUIDs must exist

## Size Limitations

### Entity

- **Attributes**: Recommended <10KB per entity
- **Parents**: Recommended <100 direct parents
- **Name**: <256 characters

### Policy

- **Conditions**: Recommended <100 operators
- **Actions**: Recommended <1000 actions per policy
- **Name**: <256 characters

### Action

- **Name**: <128 characters
- **Description**: <1024 characters

### ActionSet

- **Actions**: Recommended <1000 actions per set
- **Name**: <128 characters

## Related Documentation

- [Architecture](./architecture.md) - Overall system design
- [Authorization Flow](./authorization-flow.md) - How data model is used
- [Bytecode VM](./bytecode-vm.md) - Condition evaluation
- [Snapshot Format](./snapshot-format.md) - How data is indexed
