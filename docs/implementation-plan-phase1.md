# Implementation Plan: Phase 1 - Core Authorization Engine

**Duration**: 4-6 weeks (Weeks 1-5 of V1)
**Goal**: Build the core authorization engine with bytecode VM, snapshot generation, and all authorization operations
**Status**: 🚧 Ready to Start

---

## Overview

Phase 1 focuses on the foundational authorization logic. By the end of this phase, we'll have:
- ✅ Bytecode VM that evaluates policy conditions
- ✅ Snapshot builder that transforms graph data into optimized indexes
- ✅ All 4 authorization operations (check, list_resources, list_principals, list_actions)
- ✅ Helper functions for attribute resolution and policy management

**Entry Criteria**:
- Foundation complete (~24%): Core types, graph storage, index structure, OpCode definitions

**Exit Criteria**:
- All authorization operations working end-to-end
- Unit tests passing for all components
- Integration tests demonstrating correctness
- Ready to build services in Phase 2

---

## Week 1-2: Bytecode VM

### Objective
Build a stack-based bytecode interpreter that evaluates compiled policy conditions with proper error handling and type safety.

### Tasks

#### Task 1.1: VM Core Infrastructure (2-3 days)
**Priority**: 🔴 Critical
**Dependencies**: None (uses existing OpCode definitions)

```rust
// File: crates/arbor-bytecode/src/vm.rs

pub struct BytecodeVM {
    stack: Vec<Value>,
    instruction_pointer: usize,
}

pub struct EvaluationContext<'a> {
    pub principal: &'a IndexedEntity,
    pub resource: &'a IndexedEntity,
    pub context_attributes: &'a Attributes,
    pub snapshot: &'a IndexSnapshot,
}

pub enum Value {
    Bool(bool),
    Int(i64),
    String(String),
    Set(Vec<Value>),
    Unknown,  // Missing attribute
    Invalid,  // Type error
}

pub enum ConditionResult {
    True,
    False,
    Unknown,  // Missing data - fail closed for forbids
    Invalid,  // Runtime error - fail closed for forbids
}
```

**Implementation Steps**:
1. Create `BytecodeVM` struct with stack and instruction pointer
2. Implement `push()`, `pop()`, and `peek()` stack operations
3. Add stack underflow/overflow guards
4. Create `EvaluationContext` for variable resolution
5. Implement `Value` enum with type conversions
6. Add `ConditionResult` enum

**Tests**:
```rust
#[test]
fn test_stack_operations() {
    let mut vm = BytecodeVM::new();
    vm.push(Value::Int(42));
    assert_eq!(vm.pop(), Some(Value::Int(42)));
    assert_eq!(vm.pop(), None);  // Underflow
}

#[test]
fn test_value_type_conversions() {
    let v = Value::Int(1);
    assert!(v.as_bool().is_err());  // Type error

    let v = Value::Bool(true);
    assert_eq!(v.as_bool(), Ok(true));
}
```

**Acceptance Criteria**:
- [ ] Stack operations work correctly with guards
- [ ] Value types handle conversions and errors
- [ ] EvaluationContext struct defined

---

#### Task 1.2: Arithmetic & Comparison OpCodes (2-3 days)
**Priority**: 🔴 Critical
**Dependencies**: Task 1.1

Implement execution for:
- Arithmetic: `Add`, `Sub`, `Mul`, `Div`, `Mod`, `Neg`
- Comparison: `Eq`, `Neq`, `Lt`, `Lte`, `Gt`, `Gte`
- Literals: `PushInt`, `PushString`, `PushBool`

```rust
impl BytecodeVM {
    pub fn execute(
        &mut self,
        bytecode: &[OpCode],
        context: &EvaluationContext,
    ) -> Result<ConditionResult> {
        self.instruction_pointer = 0;

        while self.instruction_pointer < bytecode.len() {
            let opcode = &bytecode[self.instruction_pointer];

            match opcode {
                OpCode::PushInt(n) => {
                    self.stack.push(Value::Int(*n));
                }
                OpCode::Add => {
                    let b = self.pop_int()?;
                    let a = self.pop_int()?;
                    self.stack.push(Value::Int(a + b));
                }
                OpCode::Eq => {
                    let b = self.pop()?;
                    let a = self.pop()?;
                    self.stack.push(Value::Bool(a == b));
                }
                // ... more opcodes
            }

            self.instruction_pointer += 1;
        }

        // Final result
        let result = self.pop()?;
        Ok(result.as_condition_result())
    }
}
```

**Implementation Steps**:
1. Implement literal push operations (PushInt, PushString, PushBool)
2. Add helper methods: `pop_int()`, `pop_string()`, `pop_bool()`
3. Implement arithmetic operations with overflow checks
4. Implement comparison operations with type checking
5. Handle type mismatches → `ConditionResult::Invalid`
6. Handle division by zero → `ConditionResult::Invalid`

**Tests**:
```rust
#[test]
fn test_arithmetic() {
    let bytecode = vec![
        OpCode::PushInt(10),
        OpCode::PushInt(5),
        OpCode::Add,
    ];

    let mut vm = BytecodeVM::new();
    let context = /* ... */;
    let result = vm.execute(&bytecode, &context).unwrap();

    // Stack should have 15
    assert_eq!(vm.pop(), Some(Value::Int(15)));
}

#[test]
fn test_comparison() {
    let bytecode = vec![
        OpCode::PushInt(10),
        OpCode::PushInt(5),
        OpCode::Gt,  // 10 > 5 = true
    ];

    let result = execute_bytecode(&bytecode);
    assert_eq!(result, ConditionResult::True);
}

#[test]
fn test_division_by_zero() {
    let bytecode = vec![
        OpCode::PushInt(10),
        OpCode::PushInt(0),
        OpCode::Div,
    ];

    let result = execute_bytecode(&bytecode);
    assert_eq!(result, ConditionResult::Invalid);  // Error
}
```

**Acceptance Criteria**:
- [ ] All arithmetic operations work correctly
- [ ] Comparisons handle different types appropriately
- [ ] Errors produce `ConditionResult::Invalid`

---

#### Task 1.3: Boolean Logic & Control Flow (2 days)
**Priority**: 🔴 Critical
**Dependencies**: Task 1.2

Implement:
- Boolean: `And`, `Or`, `Not`
- Control: `Jump`, `JumpIfFalse`, `JumpIfTrue`

```rust
match opcode {
    OpCode::And => {
        let b = self.pop_bool()?;
        let a = self.pop_bool()?;

        // Short-circuit logic already compiled into jumps
        self.stack.push(Value::Bool(a && b));
    }
    OpCode::JumpIfFalse(offset) => {
        let condition = self.pop_bool()?;
        if !condition {
            self.instruction_pointer = *offset;
            continue;  // Skip increment
        }
    }
    // ... etc
}
```

**Implementation Steps**:
1. Implement `And`, `Or`, `Not` operations
2. Add jump instructions for control flow
3. Handle Unknown values in boolean logic:
   - `Unknown && X` → `Unknown`
   - `Unknown || True` → `True`
   - `Unknown || False` → `Unknown`
4. Add instruction pointer manipulation for jumps

**Tests**:
```rust
#[test]
fn test_boolean_logic() {
    // true && false = false
    let bytecode = vec![
        OpCode::PushBool(true),
        OpCode::PushBool(false),
        OpCode::And,
    ];

    let result = execute_bytecode(&bytecode);
    assert_eq!(result, ConditionResult::False);
}

#[test]
fn test_unknown_propagation() {
    // unknown && true = unknown
    let bytecode = vec![
        OpCode::PushUnknown,
        OpCode::PushBool(true),
        OpCode::And,
    ];

    let result = execute_bytecode(&bytecode);
    assert_eq!(result, ConditionResult::Unknown);
}
```

**Acceptance Criteria**:
- [ ] Boolean operations work correctly
- [ ] Unknown values propagate according to fail-closed semantics
- [ ] Jumps work correctly (tested with compiler integration)

---

#### Task 1.4: Variable Resolution (2-3 days)
**Priority**: 🔴 Critical
**Dependencies**: Task 1.2

Implement attribute access and variable resolution:
- `GetAttribute` - Load attribute from principal/resource/context
- `HasAttribute` - Check if attribute exists

```rust
// File: crates/arbor-bytecode/src/context.rs

impl EvaluationContext<'_> {
    pub fn resolve_attribute(&self, var_ref: &VariableRef) -> Value {
        match var_ref.scope {
            Scope::Principal => {
                self.resolve_from_entity(&self.principal.entity, &var_ref.path)
            }
            Scope::Resource => {
                self.resolve_from_entity(&self.resource.entity, &var_ref.path)
            }
            Scope::Context => {
                self.resolve_from_attributes(
                    self.context_attributes,
                    &var_ref.path
                )
            }
        }
    }

    fn resolve_from_entity(&self, entity: &Entity, path: &[String]) -> Value {
        if path.is_empty() {
            return Value::Invalid;
        }

        // Try entity attributes first
        let attributes = &entity.attributes;
        match self.resolve_path(attributes, path) {
            Some(attr_value) => self.attr_value_to_vm_value(attr_value),
            None => Value::Unknown,  // Missing attribute
        }
    }

    fn resolve_path(
        &self,
        attributes: &Attributes,
        path: &[String],
    ) -> Option<&AttributeValue> {
        let mut current = attributes;

        for (i, key) in path.iter().enumerate() {
            match current.get(key) {
                Some(AttributeValue::Object(nested)) => {
                    if i == path.len() - 1 {
                        return Some(&AttributeValue::Object(nested.clone()));
                    }
                    current = nested;
                }
                Some(value) => {
                    if i == path.len() - 1 {
                        return Some(value);
                    } else {
                        return None;  // Path continues but value isn't object
                    }
                }
                None => return None,
            }
        }

        None
    }
}
```

**Implementation Steps**:
1. Create `VariableRef` struct with scope and path
2. Implement `resolve_attribute()` in EvaluationContext
3. Add nested path resolution for objects
4. Convert AttributeValue → VM Value
5. Return `Value::Unknown` for missing attributes
6. Implement `HasAttribute` opcode

**Tests**:
```rust
#[test]
fn test_attribute_resolution() {
    let principal = Entity {
        attributes: hashmap! {
            "tier".to_string() => AttributeValue::String("gold".to_string()),
            "profile".to_string() => AttributeValue::Object(hashmap! {
                "level".to_string() => AttributeValue::Int(5),
            }),
        },
        ..Default::default()
    };

    let context = EvaluationContext { principal: &principal, /* ... */ };

    // Simple attribute
    let value = context.resolve_attribute(&VariableRef {
        scope: Scope::Principal,
        path: vec!["tier".to_string()],
    });
    assert_eq!(value, Value::String("gold".to_string()));

    // Nested attribute
    let value = context.resolve_attribute(&VariableRef {
        scope: Scope::Principal,
        path: vec!["profile".to_string(), "level".to_string()],
    });
    assert_eq!(value, Value::Int(5));

    // Missing attribute
    let value = context.resolve_attribute(&VariableRef {
        scope: Scope::Principal,
        path: vec!["missing".to_string()],
    });
    assert_eq!(value, Value::Unknown);
}
```

**Acceptance Criteria**:
- [ ] Simple attributes resolve correctly
- [ ] Nested paths work (e.g., `principal.profile.level`)
- [ ] Missing attributes return `Value::Unknown`
- [ ] `HasAttribute` opcode works

---

#### Task 1.5: Set Operations & Special Operators (2 days)
**Priority**: 🟡 Important
**Dependencies**: Task 1.2, Task 1.4

Implement:
- `In` - Element in set membership
- `Contains` - Set contains element
- `ContainsAll` - Set contains all elements
- `ContainsAny` - Set contains any element

```rust
match opcode {
    OpCode::In => {
        let set = self.pop_set()?;
        let element = self.pop()?;

        let result = set.contains(&element);
        self.stack.push(Value::Bool(result));
    }
    OpCode::ContainsAny => {
        let set_b = self.pop_set()?;
        let set_a = self.pop_set()?;

        let result = set_a.iter().any(|x| set_b.contains(x));
        self.stack.push(Value::Bool(result));
    }
    // ... etc
}
```

**Implementation Steps**:
1. Implement `Value::Set` operations
2. Add `In` operation for membership testing
3. Implement set comparison operations
4. Handle Unknown/Invalid values in sets

**Tests**:
```rust
#[test]
fn test_set_membership() {
    // "admin" in ["admin", "user"] = true
    let bytecode = vec![
        OpCode::PushString("admin".to_string()),
        OpCode::PushSet(vec![
            Value::String("admin".to_string()),
            Value::String("user".to_string()),
        ]),
        OpCode::In,
    ];

    let result = execute_bytecode(&bytecode);
    assert_eq!(result, ConditionResult::True);
}
```

**Acceptance Criteria**:
- [ ] Set operations work correctly
- [ ] `In` handles different value types
- [ ] Set comparison operations are efficient

---

#### Task 1.6: Compiler Implementation (3 days)
**Priority**: 🔴 Critical
**Dependencies**: All VM tasks

Transform AST conditions into bytecode.

```rust
// File: crates/arbor-bytecode/src/compiler.rs

pub struct BytecodeCompiler {
    bytecode: Vec<OpCode>,
}

impl BytecodeCompiler {
    pub fn compile(condition: &Condition) -> Result<Vec<OpCode>> {
        let mut compiler = Self {
            bytecode: Vec::new(),
        };

        compiler.compile_condition(condition)?;
        Ok(compiler.bytecode)
    }

    fn compile_condition(&mut self, condition: &Condition) -> Result<()> {
        match condition {
            Condition::Eq(left, right) => {
                self.compile_value_expr(left)?;
                self.compile_value_expr(right)?;
                self.bytecode.push(OpCode::Eq);
            }
            Condition::And(left, right) => {
                // Short-circuit compilation:
                // if left is false, skip right evaluation
                self.compile_condition(left)?;
                self.bytecode.push(OpCode::Dup);  // Duplicate for jump test

                let jump_offset_idx = self.bytecode.len();
                self.bytecode.push(OpCode::JumpIfFalse(0));  // Placeholder

                self.compile_condition(right)?;
                self.bytecode.push(OpCode::And);

                // Patch jump offset
                let jump_target = self.bytecode.len();
                self.bytecode[jump_offset_idx] =
                    OpCode::JumpIfFalse(jump_target);
            }
            Condition::GetAttribute(var_ref) => {
                self.bytecode.push(OpCode::GetAttribute(var_ref.clone()));
            }
            // ... more conditions
        }

        Ok(())
    }

    fn compile_value_expr(&mut self, expr: &ValueExpr) -> Result<()> {
        match expr {
            ValueExpr::Literal(value) => {
                match value {
                    AttributeValue::Int(n) => {
                        self.bytecode.push(OpCode::PushInt(*n));
                    }
                    AttributeValue::String(s) => {
                        self.bytecode.push(OpCode::PushString(s.clone()));
                    }
                    // ... more types
                }
            }
            ValueExpr::Variable(var_ref) => {
                self.bytecode.push(OpCode::GetAttribute(var_ref.clone()));
            }
        }

        Ok(())
    }
}
```

**Implementation Steps**:
1. Create `BytecodeCompiler` struct
2. Implement recursive condition compilation
3. Add short-circuit compilation for `And`/`Or`
4. Implement jump offset patching
5. Add constant folding optimization (optional)
6. Compile value expressions (literals, variables)

**Tests**:
```rust
#[test]
fn test_compile_simple_condition() {
    // principal.tier == "gold"
    let condition = Condition::Eq(
        ValueExpr::Variable(VariableRef {
            scope: Scope::Principal,
            path: vec!["tier".to_string()],
        }),
        ValueExpr::Literal(AttributeValue::String("gold".to_string())),
    );

    let bytecode = BytecodeCompiler::compile(&condition).unwrap();

    assert_eq!(bytecode, vec![
        OpCode::GetAttribute(/* ... */),
        OpCode::PushString("gold".to_string()),
        OpCode::Eq,
    ]);
}

#[test]
fn test_compile_short_circuit_and() {
    // false && <expensive>
    let condition = Condition::And(
        Box::new(Condition::Literal(false)),
        Box::new(Condition::Eq(/* complex expr */)),
    );

    let bytecode = BytecodeCompiler::compile(&condition).unwrap();

    // Should have JumpIfFalse to skip right side
    assert!(bytecode.iter().any(|op| matches!(op, OpCode::JumpIfFalse(_))));
}

#[test]
fn test_bytecode_equivalence_to_ast() {
    // Property-based test: compiled bytecode produces same result as AST
    proptest!(|(condition: Condition)| {
        let bytecode = BytecodeCompiler::compile(&condition)?;

        let ast_result = evaluate_ast(&condition, &context);
        let vm_result = execute_bytecode(&bytecode, &context);

        prop_assert_eq!(ast_result, vm_result);
    });
}
```

**Acceptance Criteria**:
- [ ] All condition types compile correctly
- [ ] Short-circuit logic works
- [ ] Jump offsets are patched correctly
- [ ] Property-based tests verify equivalence with AST

---

#### Task 1.7: Integration & Testing (2 days)
**Priority**: 🔴 Critical
**Dependencies**: All above tasks

**Implementation Steps**:
1. Write integration tests (compile + execute)
2. Add property-based tests for correctness
3. Benchmark VM performance vs AST evaluation
4. Add comprehensive error handling tests
5. Test Unknown/Invalid propagation semantics

**Tests**:
```rust
#[test]
fn test_end_to_end_condition_evaluation() {
    // Full condition: principal.tier == "gold" && resource.size > 1000
    let condition = Condition::And(
        Box::new(Condition::Eq(
            ValueExpr::Variable(/* principal.tier */),
            ValueExpr::Literal(AttributeValue::String("gold".to_string())),
        )),
        Box::new(Condition::Gt(
            ValueExpr::Variable(/* resource.size */),
            ValueExpr::Literal(AttributeValue::Int(1000)),
        )),
    );

    // Compile
    let bytecode = BytecodeCompiler::compile(&condition).unwrap();

    // Execute
    let principal = Entity {
        attributes: hashmap! {
            "tier".to_string() => AttributeValue::String("gold".to_string()),
        },
        ..Default::default()
    };

    let resource = Entity {
        attributes: hashmap! {
            "size".to_string() => AttributeValue::Int(5000),
        },
        ..Default::default()
    };

    let context = EvaluationContext {
        principal: &IndexedEntity::from(principal),
        resource: &IndexedEntity::from(resource),
        context_attributes: &Attributes::default(),
        snapshot: &snapshot,
    };

    let mut vm = BytecodeVM::new();
    let result = vm.execute(&bytecode, &context).unwrap();

    assert_eq!(result, ConditionResult::True);
}

#[test]
fn test_fail_closed_for_forbids() {
    // Unknown condition on forbid should forbid
    let condition = Condition::Eq(
        ValueExpr::Variable(/* missing attribute */),
        ValueExpr::Literal(AttributeValue::String("value".to_string())),
    );

    let bytecode = BytecodeCompiler::compile(&condition).unwrap();
    let result = execute_bytecode(&bytecode, &context);

    assert_eq!(result, ConditionResult::Unknown);

    // In authorization flow, Unknown on forbid → apply forbid
}
```

**Acceptance Criteria**:
- [ ] End-to-end tests pass for complex conditions
- [ ] Performance is 2-4x faster than AST evaluation
- [ ] Unknown/Invalid semantics work correctly
- [ ] All edge cases covered

---

### Week 1-2 Deliverables

**Code**:
- ✅ `crates/arbor-bytecode/src/vm.rs` (~400 lines)
- ✅ `crates/arbor-bytecode/src/compiler.rs` (~300 lines)
- ✅ `crates/arbor-bytecode/src/context.rs` (~200 lines)
- ✅ `crates/arbor-bytecode/tests/vm_tests.rs` (~300 lines)
- ✅ `crates/arbor-bytecode/tests/integration_tests.rs` (~200 lines)

**Total**: ~1,400 lines

**Documentation**:
- Bytecode VM usage examples
- Compilation process documentation
- Performance benchmarks

---

## Week 2-3: Snapshot Builder

### Objective
Transform mutable graph data into optimized, immutable snapshots with precomputed transitive closures and compiled policies.

### Tasks

#### Task 2.1: Transitive Closure Computation (2-3 days)
**Priority**: 🔴 Critical
**Dependencies**: None (uses existing Graph)

```rust
// File: services/arbor-indexer/src/closures.rs

pub fn compute_ancestors(
    graph: &Graph,
    entity_uuid: Uuid,
) -> Result<RoaringBitmap> {
    let mut ancestors = RoaringBitmap::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    // Get initial parents
    if let Some(entity) = graph.get_entity(&entity_uuid) {
        for parent_uuid in &entity.parents {
            queue.push_back(*parent_uuid);
        }
    }

    // BFS traversal
    while let Some(current_uuid) = queue.pop_front() {
        if visited.contains(&current_uuid) {
            continue;  // Already processed
        }
        visited.insert(current_uuid);

        // Get graph index for this entity
        if let Some(index) = graph.uuid_to_index.get(&current_uuid) {
            ancestors.insert(*index as u32);

            // Add parents to queue
            if let Some(entity) = graph.get_entity(&current_uuid) {
                for parent_uuid in &entity.parents {
                    if !visited.contains(parent_uuid) {
                        queue.push_back(*parent_uuid);
                    }
                }
            }
        }
    }

    Ok(ancestors)
}

pub fn compute_descendants(
    graph: &Graph,
    entity_uuid: Uuid,
) -> Result<RoaringBitmap> {
    let mut descendants = RoaringBitmap::new();
    let mut visited = HashSet::new();
    let mut queue = VecDeque::new();

    // Start with the entity itself
    queue.push_back(entity_uuid);

    // BFS traversal (inverse direction - follow children)
    while let Some(current_uuid) = queue.pop_front() {
        if visited.contains(&current_uuid) {
            continue;
        }
        visited.insert(current_uuid);

        if let Some(index) = graph.uuid_to_index.get(&current_uuid) {
            descendants.insert(*index as u32);

            // Add children to queue
            if let Some(entity) = graph.get_entity(&current_uuid) {
                for child_uuid in &entity.children {
                    if !visited.contains(child_uuid) {
                        queue.push_back(*child_uuid);
                    }
                }
            }
        }
    }

    Ok(descendants)
}

pub fn detect_cycles(graph: &Graph, entity_uuid: Uuid) -> bool {
    let mut visited = HashSet::new();
    let mut rec_stack = HashSet::new();

    dfs_cycle_detect(graph, entity_uuid, &mut visited, &mut rec_stack)
}

fn dfs_cycle_detect(
    graph: &Graph,
    uuid: Uuid,
    visited: &mut HashSet<Uuid>,
    rec_stack: &mut HashSet<Uuid>,
) -> bool {
    visited.insert(uuid);
    rec_stack.insert(uuid);

    if let Some(entity) = graph.get_entity(&uuid) {
        for parent_uuid in &entity.parents {
            if !visited.contains(parent_uuid) {
                if dfs_cycle_detect(graph, *parent_uuid, visited, rec_stack) {
                    return true;  // Cycle detected
                }
            } else if rec_stack.contains(parent_uuid) {
                return true;  // Back edge = cycle
            }
        }
    }

    rec_stack.remove(&uuid);
    false
}
```

**Implementation Steps**:
1. Implement BFS for ancestor computation
2. Implement BFS for descendant computation
3. Add cycle detection (should never happen if graph validation works)
4. Use RoaringBitmap for efficient storage
5. Handle disconnected components
6. Add memoization/caching for repeated computations

**Tests**:
```rust
#[test]
fn test_compute_ancestors() {
    // Graph: user1 -> team1 -> org1
    let mut graph = Graph::new();
    let org1 = graph.add_entity(Entity::new("org1"));
    let team1 = graph.add_entity(Entity::new("team1").with_parent(org1));
    let user1 = graph.add_entity(Entity::new("user1").with_parent(team1));

    let ancestors = compute_ancestors(&graph, user1).unwrap();

    // user1's ancestors: team1, org1
    assert!(ancestors.contains(team1_index));
    assert!(ancestors.contains(org1_index));
    assert_eq!(ancestors.len(), 2);
}

#[test]
fn test_compute_descendants() {
    // Graph: org1 -> team1 -> user1
    let descendants = compute_descendants(&graph, org1).unwrap();

    // org1's descendants: org1, team1, user1
    assert!(descendants.contains(org1_index));
    assert!(descendants.contains(team1_index));
    assert!(descendants.contains(user1_index));
    assert_eq!(descendants.len(), 3);
}

#[test]
fn test_multiple_parents() {
    // Diamond: user1 -> [team1, team2] -> org1
    let mut graph = Graph::new();
    let org1 = graph.add_entity(Entity::new("org1"));
    let team1 = graph.add_entity(Entity::new("team1").with_parent(org1));
    let team2 = graph.add_entity(Entity::new("team2").with_parent(org1));
    let user1 = graph.add_entity(Entity::new("user1")
        .with_parent(team1)
        .with_parent(team2));

    let ancestors = compute_ancestors(&graph, user1).unwrap();

    // Should include both paths but org1 only once
    assert_eq!(ancestors.len(), 3);  // team1, team2, org1
}
```

**Acceptance Criteria**:
- [ ] Ancestors computed correctly for all entities
- [ ] Descendants computed correctly for all entities
- [ ] Multiple parents handled (diamond pattern)
- [ ] Cycle detection works (shouldn't find any in valid graphs)
- [ ] Performance is acceptable (< 1ms per entity for typical hierarchies)

---

#### Task 2.2: Snapshot Builder Core (3 days)
**Priority**: 🔴 Critical
**Dependencies**: Task 2.1, Week 1-2 (Bytecode Compiler)

```rust
// File: services/arbor-indexer/src/snapshot_builder.rs

pub struct SnapshotBuilder {
    graph: Arc<Graph>,
    compiler: BytecodeCompiler,
}

impl SnapshotBuilder {
    pub fn new(graph: Arc<Graph>) -> Self {
        Self {
            graph,
            compiler: BytecodeCompiler::new(),
        }
    }

    pub fn build_snapshot(&self) -> Result<Snapshot> {
        // Phase 1: Build UUID ↔ Index mappings
        let mut uuid_to_index = HashMap::new();
        let mut index_to_uuid = Vec::new();

        for (idx, entity) in self.graph.entities.iter().enumerate() {
            if let Some(entity) = entity {
                uuid_to_index.insert(entity.id, idx);
                index_to_uuid.push(entity.id);
            }
        }

        // Phase 2: Build IndexedEntities with closures
        let indexed_entities = self.build_indexed_entities(&uuid_to_index)?;

        // Phase 3: Compile and index policies
        let indexed_policies = self.build_indexed_policies(&uuid_to_index)?;

        // Phase 4: Build specialized indexes
        let entity_type_indexes = self.build_entity_type_indexes(&indexed_entities);
        let action_policy_indexes = self.build_action_indexes(&indexed_policies);

        // Phase 5: Compute checksum
        let checksum = self.compute_checksum(&indexed_entities, &indexed_policies)?;

        Ok(Snapshot {
            version: self.generate_version(),
            created_at: Utc::now().timestamp(),
            uuid_to_index,
            index_to_uuid,
            indexed_entities,
            indexed_policies,
            entity_type_indexes,
            action_policy_indexes,
            checksum,
        })
    }

    fn build_indexed_entities(
        &self,
        uuid_to_index: &HashMap<Uuid, usize>,
    ) -> Result<Vec<IndexedEntity>> {
        let mut indexed_entities = Vec::new();

        for entity in self.graph.get_all_entities() {
            let ancestors = compute_ancestors(&self.graph, entity.id)?;
            let descendants = compute_descendants(&self.graph, entity.id)?;

            let indexed_entity = IndexedEntity {
                entity: entity.clone(),
                index: uuid_to_index[&entity.id],
                ancestors,
                descendants,
            };

            indexed_entities.push(indexed_entity);
        }

        Ok(indexed_entities)
    }

    fn build_indexed_policies(
        &self,
        uuid_to_index: &HashMap<Uuid, usize>,
    ) -> Result<Vec<IndexedPolicy>> {
        let mut indexed_policies = Vec::new();

        for policy in self.graph.get_all_policies() {
            // Validate policy before indexing (skip if invalid)
            if let Err(e) = self.validate_policy(&policy) {
                log::warn!("Skipping invalid policy {}: {}", policy.id, e);
                metrics::counter!("arbor.indexer.invalid_policies_skipped", 1);
                continue;
            }

            // Compile conditions to bytecode
            let compiled_condition = if let Some(condition) = &policy.conditions {
                Some(self.compiler.compile(condition)?)
            } else {
                None
            };

            // Expand action sets to individual actions
            let expanded_actions = self.expand_action_sets(&policy)?;

            let indexed_policy = IndexedPolicy {
                policy: policy.clone(),
                index: indexed_policies.len(),
                compiled_condition,
                expanded_actions,
            };

            indexed_policies.push(indexed_policy);
        }

        Ok(indexed_policies)
    }

    fn validate_policy(&self, policy: &Policy) -> Result<()> {
        // Re-validate entity references
        match &policy.principal {
            PolicyTarget::Entity(uuid) | PolicyTarget::EntityWithDescendants(uuid) => {
                if !self.graph.entity_exists(uuid) {
                    return Err(Error::InvalidPolicyTarget {
                        policy_id: policy.id,
                        target_type: "principal",
                        entity_id: *uuid,
                    });
                }
            }
            _ => {}
        }

        match &policy.resource {
            PolicyTarget::Entity(uuid) | PolicyTarget::EntityWithDescendants(uuid) => {
                if !self.graph.entity_exists(uuid) {
                    return Err(Error::InvalidPolicyTarget {
                        policy_id: policy.id,
                        target_type: "resource",
                        entity_id: *uuid,
                    });
                }
            }
            _ => {}
        }

        // Validate action references
        for action_id in &policy.actions {
            if !self.graph.action_exists(action_id) {
                return Err(Error::InvalidAction {
                    policy_id: policy.id,
                    action_id: *action_id,
                });
            }
        }

        Ok(())
    }

    fn expand_action_sets(&self, policy: &Policy) -> Result<Vec<ActionId>> {
        let mut actions = policy.actions.clone();

        for action_set_id in &policy.action_sets {
            if let Some(action_set) = self.graph.get_action_set(action_set_id) {
                actions.extend(&action_set.actions);
            } else {
                return Err(Error::InvalidActionSet {
                    policy_id: policy.id,
                    action_set_id: *action_set_id,
                });
            }
        }

        // Deduplicate
        actions.sort();
        actions.dedup();

        Ok(actions)
    }

    fn compute_checksum(
        &self,
        indexed_entities: &[IndexedEntity],
        indexed_policies: &[IndexedPolicy],
    ) -> Result<[u8; 32]> {
        use sha2::{Sha256, Digest};

        let mut hasher = Sha256::new();

        // Hash all entities
        for entity in indexed_entities {
            let serialized = bincode::serialize(&entity.entity)?;
            hasher.update(&serialized);
        }

        // Hash all policies
        for policy in indexed_policies {
            let serialized = bincode::serialize(&policy.policy)?;
            hasher.update(&serialized);
        }

        let result = hasher.finalize();
        let mut checksum = [0u8; 32];
        checksum.copy_from_slice(&result);

        Ok(checksum)
    }
}
```

**Implementation Steps**:
1. Create `SnapshotBuilder` struct
2. Build UUID ↔ index mappings
3. Generate `IndexedEntity` with closures for all entities
4. Compile policy conditions to bytecode
5. Expand action sets to individual actions
6. Re-validate policies (skip invalid ones with alerts)
7. Compute snapshot checksum (SHA256)
8. Add version generation (monotonic counter)

**Tests**:
```rust
#[test]
fn test_snapshot_builder_basic() {
    let mut graph = Graph::new();

    // Add entities
    let org = graph.add_entity(Entity::new("org").with_type("organization"));
    let user = graph.add_entity(Entity::new("user").with_type("user").with_parent(org));

    // Add policy
    let policy = Policy {
        name: "Test Policy".to_string(),
        policy_type: PolicyType::Permit,
        principal: PolicyTarget::EntityType("user".to_string()),
        resource: PolicyTarget::All,
        actions: vec![read_action],
        conditions: None,
        ..Default::default()
    };
    graph.add_policy(policy);

    // Build snapshot
    let builder = SnapshotBuilder::new(Arc::new(graph));
    let snapshot = builder.build_snapshot().unwrap();

    // Verify
    assert_eq!(snapshot.indexed_entities.len(), 2);
    assert_eq!(snapshot.indexed_policies.len(), 1);
    assert!(snapshot.uuid_to_index.contains_key(&user));

    // Check ancestors
    let user_entity = &snapshot.indexed_entities[snapshot.uuid_to_index[&user]];
    assert!(user_entity.ancestors.contains(snapshot.uuid_to_index[&org] as u32));
}

#[test]
fn test_skip_invalid_policies() {
    let mut graph = Graph::new();

    // Add policy referencing non-existent entity
    let policy = Policy {
        principal: PolicyTarget::Entity(Uuid::new_v4()),  // Doesn't exist
        ..Default::default()
    };
    graph.add_policy(policy);

    // Build snapshot (should skip invalid policy)
    let builder = SnapshotBuilder::new(Arc::new(graph));
    let snapshot = builder.build_snapshot().unwrap();

    // Invalid policy should be skipped
    assert_eq!(snapshot.indexed_policies.len(), 0);
}
```

**Acceptance Criteria**:
- [ ] Snapshot generation works for complex graphs
- [ ] Invalid policies are skipped with warnings
- [ ] Checksums are stable (same input → same checksum)
- [ ] All closures computed correctly

---

#### Task 2.3: Specialized Indexes (2 days)
**Priority**: 🟡 Important
**Dependencies**: Task 2.2

Build optimized indexes for fast query patterns.

```rust
fn build_entity_type_indexes(
    &self,
    indexed_entities: &[IndexedEntity],
) -> HashMap<EntityTypeId, RoaringBitmap> {
    let mut indexes = HashMap::new();

    for indexed_entity in indexed_entities {
        let entity_type = &indexed_entity.entity.entity_type;

        indexes
            .entry(entity_type.clone())
            .or_insert_with(RoaringBitmap::new)
            .insert(indexed_entity.index as u32);
    }

    indexes
}

fn build_action_indexes(
    &self,
    indexed_policies: &[IndexedPolicy],
) -> HashMap<ActionId, Vec<usize>> {
    let mut indexes: HashMap<ActionId, Vec<usize>> = HashMap::new();

    for indexed_policy in indexed_policies {
        for action_id in &indexed_policy.expanded_actions {
            indexes
                .entry(*action_id)
                .or_insert_with(Vec::new)
                .push(indexed_policy.index);
        }
    }

    indexes
}
```

**Implementation Steps**:
1. Build entity type → entities bitmap index
2. Build action → policies vector index
3. Add optional principal type index (for list_principals)
4. Add optional resource type index (for list_resources)

**Tests**:
```rust
#[test]
fn test_entity_type_index() {
    // Multiple entities of same type
    let snapshot = build_test_snapshot();

    let user_type = EntityTypeId::from("user");
    let user_indexes = &snapshot.entity_type_indexes[&user_type];

    // Should include all user entities
    assert_eq!(user_indexes.len(), 3);
}

#[test]
fn test_action_policy_index() {
    let snapshot = build_test_snapshot();

    let read_action = ActionId::from("read");
    let policies = &snapshot.action_policy_indexes[&read_action];

    // Should include all policies with read action
    assert_eq!(policies.len(), 5);
}
```

**Acceptance Criteria**:
- [ ] Entity type indexes work correctly
- [ ] Action indexes work correctly
- [ ] Indexes are used in authorization operations

---

#### Task 2.4: Batching Strategy (1-2 days)
**Priority**: 🟡 Important
**Dependencies**: Task 2.2

Implement configurable batching to control snapshot generation frequency.

```rust
// File: services/arbor-indexer/src/batching.rs

pub enum BatchStrategy {
    /// Generate snapshot after N changes
    CountBased { threshold: usize },

    /// Generate snapshot every N seconds
    TimeBased { interval: Duration },

    /// Generate snapshot N seconds after last change (debounced)
    Debounced { delay: Duration },

    /// Hybrid: time-based with minimum change threshold
    Hybrid {
        interval: Duration,
        min_changes: usize,
    },
}

pub struct BatchTrigger {
    strategy: BatchStrategy,
    pending_changes: usize,
    last_snapshot: Instant,
    last_change: Option<Instant>,
}

impl BatchTrigger {
    pub fn should_generate_snapshot(&self) -> bool {
        match &self.strategy {
            BatchStrategy::CountBased { threshold } => {
                self.pending_changes >= *threshold
            }
            BatchStrategy::TimeBased { interval } => {
                self.last_snapshot.elapsed() >= *interval
            }
            BatchStrategy::Debounced { delay } => {
                if let Some(last_change) = self.last_change {
                    last_change.elapsed() >= *delay
                } else {
                    false
                }
            }
            BatchStrategy::Hybrid { interval, min_changes } => {
                self.last_snapshot.elapsed() >= *interval
                    && self.pending_changes >= *min_changes
            }
        }
    }

    pub fn record_change(&mut self) {
        self.pending_changes += 1;
        self.last_change = Some(Instant::now());
    }

    pub fn reset_after_snapshot(&mut self) {
        self.pending_changes = 0;
        self.last_snapshot = Instant::now();
        self.last_change = None;
    }
}
```

**Implementation Steps**:
1. Define `BatchStrategy` enum
2. Implement `BatchTrigger` logic
3. Add configuration options
4. Integrate with indexer main loop (Phase 2)
5. Add metrics for batch behavior

**Tests**:
```rust
#[test]
fn test_count_based_batching() {
    let mut trigger = BatchTrigger::new(BatchStrategy::CountBased { threshold: 10 });

    for _ in 0..9 {
        trigger.record_change();
        assert!(!trigger.should_generate_snapshot());
    }

    trigger.record_change();  // 10th change
    assert!(trigger.should_generate_snapshot());
}

#[test]
fn test_debounced_batching() {
    let mut trigger = BatchTrigger::new(BatchStrategy::Debounced {
        delay: Duration::from_millis(100),
    });

    trigger.record_change();
    assert!(!trigger.should_generate_snapshot());

    std::thread::sleep(Duration::from_millis(150));
    assert!(trigger.should_generate_snapshot());
}
```

**Acceptance Criteria**:
- [ ] All batch strategies work correctly
- [ ] Configuration is flexible
- [ ] Default strategy chosen (recommend: time-based with 30s interval)

---

### Week 2-3 Deliverables

**Code**:
- ✅ `services/arbor-indexer/src/snapshot_builder.rs` (~400 lines)
- ✅ `services/arbor-indexer/src/closures.rs` (~300 lines)
- ✅ `services/arbor-indexer/src/batching.rs` (~200 lines)
- ✅ `services/arbor-indexer/tests/snapshot_tests.rs` (~300 lines)

**Total**: ~1,200 lines

---

## Week 3-4: Authorization Operations

### Objective
Implement all four authorization operations: check(), list_resources(), list_principals(), list_actions().

### Tasks

#### Task 3.1: check() Operation (3 days)
**Priority**: 🔴 Critical
**Dependencies**: Week 1-2 (VM), Week 2-3 (Snapshot)

```rust
// File: services/arbor-authorizer/src/check.rs

pub struct CheckRequest {
    pub principal_id: Uuid,
    pub action_id: ActionId,
    pub resource_id: Uuid,
    pub context: Attributes,
}

pub struct CheckResponse {
    pub decision: Decision,
    pub reason: Option<Reason>,
    pub snapshot_version: u64,
}

pub enum Decision {
    Permit,
    Deny,
}

pub enum Reason {
    PermittedBy { policy_id: Uuid, policy_name: String },
    ForbiddenBy { policy_id: Uuid, policy_name: String },
    PolicyEvaluationError { policy_id: Uuid, message: String },
    NoApplicablePolicy,
}

pub fn check(
    snapshot: &Snapshot,
    request: &CheckRequest,
) -> Result<CheckResponse> {
    // Get applicable policies
    let applicable_policies = get_applicable_policies(
        snapshot,
        request.principal_id,
        request.action_id,
        request.resource_id,
    )?;

    // Split into 4 categories
    let (
        unconditional_forbid,
        conditional_forbid,
        unconditional_permit,
        conditional_permit,
    ) = split_policies(applicable_policies);

    // Phase 1: Check unconditional forbids
    if !unconditional_forbid.is_empty() {
        let policy = &unconditional_forbid[0];
        return Ok(CheckResponse {
            decision: Decision::Deny,
            reason: Some(Reason::ForbiddenBy {
                policy_id: policy.policy.id,
                policy_name: policy.policy.name.clone(),
            }),
            snapshot_version: snapshot.version,
        });
    }

    // Phase 2: Evaluate conditional forbids
    let eval_context = EvaluationContext {
        principal: get_indexed_entity(snapshot, request.principal_id)?,
        resource: get_indexed_entity(snapshot, request.resource_id)?,
        context_attributes: &request.context,
        snapshot,
    };

    for policy in conditional_forbid {
        match evaluate_policy(policy, &eval_context)? {
            ConditionResult::True | ConditionResult::Unknown | ConditionResult::Invalid => {
                // Fail closed: forbid on any uncertainty or error
                return Ok(CheckResponse {
                    decision: Decision::Deny,
                    reason: Some(Reason::ForbiddenBy {
                        policy_id: policy.policy.id,
                        policy_name: policy.policy.name.clone(),
                    }),
                    snapshot_version: snapshot.version,
                });
            }
            ConditionResult::False => {
                // Don't apply this forbid, continue checking
                continue;
            }
        }
    }

    // Phase 3: Check unconditional permits
    if !unconditional_permit.is_empty() {
        let policy = &unconditional_permit[0];
        return Ok(CheckResponse {
            decision: Decision::Permit,
            reason: Some(Reason::PermittedBy {
                policy_id: policy.policy.id,
                policy_name: policy.policy.name.clone(),
            }),
            snapshot_version: snapshot.version,
        });
    }

    // Phase 4: Evaluate conditional permits
    for policy in conditional_permit {
        match evaluate_policy(policy, &eval_context)? {
            ConditionResult::True => {
                return Ok(CheckResponse {
                    decision: Decision::Permit,
                    reason: Some(Reason::PermittedBy {
                        policy_id: policy.policy.id,
                        policy_name: policy.policy.name.clone(),
                    }),
                    snapshot_version: snapshot.version,
                });
            }
            ConditionResult::False | ConditionResult::Unknown | ConditionResult::Invalid => {
                // Don't grant access on false, unknown, or error
                continue;
            }
        }
    }

    // Default deny
    Ok(CheckResponse {
        decision: Decision::Deny,
        reason: Some(Reason::NoApplicablePolicy),
        snapshot_version: snapshot.version,
    })
}

fn get_applicable_policies(
    snapshot: &Snapshot,
    principal_id: Uuid,
    action_id: ActionId,
    resource_id: Uuid,
) -> Result<Vec<&IndexedPolicy>> {
    // Get policies for this action
    let policy_indexes = snapshot.action_policy_indexes
        .get(&action_id)
        .ok_or(Error::ActionNotFound(action_id))?;

    let principal_index = snapshot.uuid_to_index
        .get(&principal_id)
        .ok_or(Error::EntityNotFound(principal_id))?;

    let resource_index = snapshot.uuid_to_index
        .get(&resource_id)
        .ok_or(Error::EntityNotFound(resource_id))?;

    let principal_entity = &snapshot.indexed_entities[*principal_index];
    let resource_entity = &snapshot.indexed_entities[*resource_index];

    let mut applicable = Vec::new();

    for &policy_idx in policy_indexes {
        let policy = &snapshot.indexed_policies[policy_idx];

        // Check if policy applies to principal
        if !policy_matches_target(&policy.policy.principal, principal_entity) {
            continue;
        }

        // Check if policy applies to resource
        if !policy_matches_target(&policy.policy.resource, resource_entity) {
            continue;
        }

        applicable.push(policy);
    }

    Ok(applicable)
}

fn policy_matches_target(
    target: &PolicyTarget,
    entity: &IndexedEntity,
) -> bool {
    match target {
        PolicyTarget::All => true,
        PolicyTarget::Entity(uuid) => entity.entity.id == *uuid,
        PolicyTarget::EntityWithDescendants(uuid) => {
            entity.entity.id == *uuid || entity.ancestors.contains(/* uuid index */)
        }
        PolicyTarget::EntityType(type_id) => entity.entity.entity_type == *type_id,
    }
}

fn evaluate_policy(
    policy: &IndexedPolicy,
    context: &EvaluationContext,
) -> Result<ConditionResult> {
    if let Some(bytecode) = &policy.compiled_condition {
        let mut vm = BytecodeVM::new();
        vm.execute(bytecode, context)
    } else {
        // No condition = always true
        Ok(ConditionResult::True)
    }
}
```

**Implementation Steps**:
1. Create request/response types
2. Implement `get_applicable_policies()`
3. Implement policy target matching (Entity, EntityWithDescendants, etc.)
4. Implement 4-phase evaluation with short-circuiting
5. Add fail-closed logic for Unknown/Invalid
6. Integrate bytecode VM for condition evaluation
7. Return decision with reason for debugging

**Tests**:
```rust
#[test]
fn test_check_unconditional_permit() {
    let snapshot = build_test_snapshot();

    let request = CheckRequest {
        principal_id: user1,
        action_id: read_action,
        resource_id: doc1,
        context: Attributes::default(),
    };

    let response = check(&snapshot, &request).unwrap();

    assert_eq!(response.decision, Decision::Permit);
    assert!(matches!(response.reason, Some(Reason::PermittedBy { .. })));
}

#[test]
fn test_check_forbid_precedence() {
    // Both permit and forbid apply
    let snapshot = build_test_snapshot_with_conflict();

    let response = check(&snapshot, &request).unwrap();

    // Forbid should win
    assert_eq!(response.decision, Decision::Deny);
    assert!(matches!(response.reason, Some(Reason::ForbiddenBy { .. })));
}

#[test]
fn test_check_conditional_with_attributes() {
    // Policy: permit if principal.tier == "gold"
    let snapshot = build_test_snapshot();

    let user = Entity {
        attributes: hashmap! {
            "tier".to_string() => AttributeValue::String("gold".to_string()),
        },
        ..Default::default()
    };

    let response = check(&snapshot, &request).unwrap();

    assert_eq!(response.decision, Decision::Permit);
}

#[test]
fn test_check_fail_closed_on_unknown() {
    // Policy: forbid if principal.tier == "banned"
    // But principal has no "tier" attribute (Unknown)
    let snapshot = build_test_snapshot();

    let user = Entity {
        attributes: HashMap::new(),  // No tier
        ..Default::default()
    };

    let response = check(&snapshot, &request).unwrap();

    // Should forbid (fail closed)
    assert_eq!(response.decision, Decision::Deny);
}

#[test]
fn test_check_default_deny() {
    // No applicable policies
    let snapshot = build_test_snapshot();

    let response = check(&snapshot, &request).unwrap();

    assert_eq!(response.decision, Decision::Deny);
    assert!(matches!(response.reason, Some(Reason::NoApplicablePolicy)));
}
```

**Acceptance Criteria**:
- [ ] check() returns correct decisions for all scenarios
- [ ] Forbid precedence works (forbid overrides permit)
- [ ] Fail-closed logic works (Unknown/Invalid → deny on forbids)
- [ ] Default deny when no policies apply
- [ ] Reasons provided for debugging
- [ ] Performance target: <1ms p99 for typical requests

---

#### Task 3.2: list_resources() Operation (2-3 days)
**Priority**: 🔴 Critical
**Dependencies**: Task 3.1

```rust
// File: services/arbor-authorizer/src/list.rs

pub struct ListResourcesRequest {
    pub principal_id: Uuid,
    pub action_id: ActionId,
    pub resource_type: Option<EntityTypeId>,
    pub context: Attributes,
    pub options: ListOptions,
}

pub struct ListOptions {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

pub struct ListResourcesResponse {
    pub resources: Vec<Uuid>,
    pub total_count: usize,
    pub snapshot_version: u64,
}

pub fn list_resources(
    snapshot: &Snapshot,
    request: &ListResourcesRequest,
) -> Result<ListResourcesResponse> {
    // Phase 1: Bitmap operations for unconditional policies
    let mut allowed = RoaringBitmap::new();
    let mut forbidden = RoaringBitmap::new();

    let applicable_policies = get_policies_for_principal_action(
        snapshot,
        request.principal_id,
        request.action_id,
    )?;

    let (
        unconditional_forbid,
        conditional_forbid,
        unconditional_permit,
        conditional_permit,
    ) = split_policies(applicable_policies);

    // Apply unconditional policies via bitmap operations
    for policy in unconditional_forbid {
        let target_bitmap = policy_target_to_bitmap(snapshot, &policy.policy.resource)?;
        forbidden |= target_bitmap;
    }

    for policy in unconditional_permit {
        let target_bitmap = policy_target_to_bitmap(snapshot, &policy.policy.resource)?;
        allowed |= target_bitmap;
    }

    // Remove forbidden from allowed
    allowed -= &forbidden;

    // Filter by resource type if specified
    if let Some(resource_type) = &request.resource_type {
        if let Some(type_bitmap) = snapshot.entity_type_indexes.get(resource_type) {
            allowed &= type_bitmap;
        } else {
            allowed.clear();  // Type doesn't exist
        }
    }

    // Phase 2: Evaluate conditional policies for remaining resources
    let eval_context_base = EvaluationContext {
        principal: get_indexed_entity(snapshot, request.principal_id)?,
        resource: /* placeholder */,
        context_attributes: &request.context,
        snapshot,
    };

    // Check conditional forbids
    for resource_index in allowed.iter() {
        let resource = &snapshot.indexed_entities[resource_index as usize];

        for policy in &conditional_forbid {
            if !policy_matches_target(&policy.policy.resource, resource) {
                continue;
            }

            let mut eval_context = eval_context_base.clone();
            eval_context.resource = resource;

            match evaluate_policy(policy, &eval_context)? {
                ConditionResult::True | ConditionResult::Unknown | ConditionResult::Invalid => {
                    // Forbid this resource
                    forbidden.insert(resource_index);
                    break;
                }
                ConditionResult::False => continue,
            }
        }
    }

    allowed -= &forbidden;

    // Check conditional permits for resources not yet allowed
    let mut additional_allowed = RoaringBitmap::new();

    // Optimization: only check resources that match target and aren't already allowed/forbidden
    let potential_resources = get_potential_resources(snapshot, &conditional_permit)?;
    potential_resources -= &allowed;
    potential_resources -= &forbidden;

    for resource_index in potential_resources.iter() {
        let resource = &snapshot.indexed_entities[resource_index as usize];

        for policy in &conditional_permit {
            if !policy_matches_target(&policy.policy.resource, resource) {
                continue;
            }

            let mut eval_context = eval_context_base.clone();
            eval_context.resource = resource;

            match evaluate_policy(policy, &eval_context)? {
                ConditionResult::True => {
                    additional_allowed.insert(resource_index);
                    break;
                }
                _ => continue,
            }
        }
    }

    allowed |= additional_allowed;

    // Convert to UUIDs
    let total_count = allowed.len() as usize;
    let offset = request.options.offset.unwrap_or(0);
    let limit = request.options.limit.unwrap_or(total_count);

    let resources: Vec<Uuid> = allowed
        .iter()
        .skip(offset)
        .take(limit)
        .map(|idx| snapshot.index_to_uuid[idx as usize])
        .collect();

    Ok(ListResourcesResponse {
        resources,
        total_count,
        snapshot_version: snapshot.version,
    })
}

fn policy_target_to_bitmap(
    snapshot: &Snapshot,
    target: &PolicyTarget,
) -> Result<RoaringBitmap> {
    let mut bitmap = RoaringBitmap::new();

    match target {
        PolicyTarget::All => {
            // All entities
            for i in 0..snapshot.indexed_entities.len() {
                bitmap.insert(i as u32);
            }
        }
        PolicyTarget::Entity(uuid) => {
            if let Some(&index) = snapshot.uuid_to_index.get(uuid) {
                bitmap.insert(index as u32);
            }
        }
        PolicyTarget::EntityWithDescendants(uuid) => {
            if let Some(&index) = snapshot.uuid_to_index.get(uuid) {
                let entity = &snapshot.indexed_entities[index];
                bitmap.insert(index as u32);
                bitmap |= &entity.descendants;
            }
        }
        PolicyTarget::EntityType(type_id) => {
            if let Some(type_bitmap) = snapshot.entity_type_indexes.get(type_id) {
                bitmap = type_bitmap.clone();
            }
        }
    }

    Ok(bitmap)
}
```

**Implementation Steps**:
1. Create request/response types
2. Implement Phase 1: Bitmap operations for unconditional policies
3. Implement Phase 2: Conditional evaluation for residuals
4. Add pagination support
5. Optimize: only evaluate conditional policies for matching resources
6. Add resource type filtering

**Tests**:
```rust
#[test]
fn test_list_resources_unconditional() {
    // User can read all documents (unconditional)
    let snapshot = build_test_snapshot();

    let request = ListResourcesRequest {
        principal_id: user1,
        action_id: read_action,
        resource_type: Some("document".into()),
        context: Attributes::default(),
        options: ListOptions::default(),
    };

    let response = list_resources(&snapshot, &request).unwrap();

    assert_eq!(response.resources.len(), 5);  // 5 documents
}

#[test]
fn test_list_resources_with_forbids() {
    // User can read all documents except secret ones
    let snapshot = build_test_snapshot();

    let response = list_resources(&snapshot, &request).unwrap();

    // Should exclude secret documents
    assert!(!response.resources.contains(&secret_doc1));
}

#[test]
fn test_list_resources_conditional() {
    // User can read documents where document.owner == user.id
    let snapshot = build_test_snapshot();

    let response = list_resources(&snapshot, &request).unwrap();

    // Should only include owned documents
    assert_eq!(response.resources.len(), 2);
}

#[test]
fn test_list_resources_pagination() {
    let snapshot = build_test_snapshot();

    let request = ListResourcesRequest {
        options: ListOptions {
            limit: Some(10),
            offset: Some(5),
        },
        ..default_request()
    };

    let response = list_resources(&snapshot, &request).unwrap();

    assert_eq!(response.resources.len(), 10);
    assert_eq!(response.total_count, 50);  // Total available
}
```

**Acceptance Criteria**:
- [ ] Returns all accessible resources correctly
- [ ] Forbids are respected (excluded from results)
- [ ] Conditional policies evaluated correctly
- [ ] Pagination works
- [ ] Performance target: <10ms p99 for 10K resources

---

#### Task 3.3: list_principals() Operation (1-2 days)
**Priority**: 🟡 Important
**Dependencies**: Task 3.2

Similar to list_resources() but inverted (given resource + action, find principals).

```rust
pub fn list_principals(
    snapshot: &Snapshot,
    request: &ListPrincipalsRequest,
) -> Result<ListPrincipalsResponse> {
    // Symmetric to list_resources()
    // Phase 1: Bitmap operations for unconditional policies
    // Phase 2: Conditional evaluation
    // ...
}
```

**Implementation Steps**:
1. Copy and adapt list_resources() logic
2. Invert principal/resource roles
3. Filter by principal type if requested

**Tests**:
```rust
#[test]
fn test_list_principals() {
    // Who can read doc1?
    let snapshot = build_test_snapshot();

    let request = ListPrincipalsRequest {
        resource_id: doc1,
        action_id: read_action,
        principal_type: Some("user".into()),
        context: Attributes::default(),
        options: ListOptions::default(),
    };

    let response = list_principals(&snapshot, &request).unwrap();

    // Should include users with read permission
    assert!(response.principals.contains(&user1));
}
```

**Acceptance Criteria**:
- [ ] Returns all principals with access
- [ ] Forbids respected
- [ ] Conditional policies work
- [ ] Performance similar to list_resources()

---

#### Task 3.4: list_actions() Operation (1 day)
**Priority**: 🟡 Important
**Dependencies**: Task 3.1

Simpler: check all actions for a principal + resource pair.

```rust
pub fn list_actions(
    snapshot: &Snapshot,
    request: &ListActionsRequest,
) -> Result<ListActionsResponse> {
    let mut allowed_actions = Vec::new();

    // Get all actions
    let all_actions = get_all_actions(snapshot);

    for action_id in all_actions {
        let check_request = CheckRequest {
            principal_id: request.principal_id,
            action_id: *action_id,
            resource_id: request.resource_id,
            context: request.context.clone(),
        };

        let response = check(snapshot, &check_request)?;

        if response.decision == Decision::Permit {
            allowed_actions.push(*action_id);
        }
    }

    Ok(ListActionsResponse {
        actions: allowed_actions,
        snapshot_version: snapshot.version,
    })
}
```

**Implementation Steps**:
1. Get all actions from snapshot
2. Call check() for each action
3. Return permitted actions
4. Optimize: batch evaluations where possible

**Tests**:
```rust
#[test]
fn test_list_actions() {
    // What can user1 do with doc1?
    let snapshot = build_test_snapshot();

    let request = ListActionsRequest {
        principal_id: user1,
        resource_id: doc1,
        context: Attributes::default(),
    };

    let response = list_actions(&snapshot, &request).unwrap();

    // User can read and comment, but not edit
    assert!(response.actions.contains(&read_action));
    assert!(response.actions.contains(&comment_action));
    assert!(!response.actions.contains(&edit_action));
}
```

**Acceptance Criteria**:
- [ ] Returns all permitted actions
- [ ] Forbids respected
- [ ] Performance acceptable (< 5ms for ~20 actions)

---

### Week 3-4 Deliverables

**Code**:
- ✅ `services/arbor-authorizer/src/check.rs` (~400 lines)
- ✅ `services/arbor-authorizer/src/list.rs` (~500 lines)
- ✅ `services/arbor-authorizer/src/evaluator.rs` (~200 lines)
- ✅ `services/arbor-authorizer/tests/authorization_tests.rs` (~400 lines)

**Total**: ~1,500 lines

---

## Week 4-5: Helper Functions

### Objective
Complete remaining helper functions needed for full functionality.

### Tasks

#### Task 4.1: Attribute Path Resolution (1 day)
**Priority**: 🟡 Important

Already implemented in Task 1.4 (Variable Resolution), but add comprehensive tests and edge cases.

**Implementation Steps**:
1. Enhance `resolve_path()` with better error messages
2. Add support for array indexing (optional, V2+)
3. Handle null/undefined values explicitly

**Tests**:
```rust
#[test]
fn test_deep_nested_attributes() {
    // principal.profile.settings.notifications.email
    let attributes = /* deeply nested */;

    let value = resolve_path(&attributes, &["profile", "settings", "notifications", "email"]);
    assert!(value.is_some());
}

#[test]
fn test_attribute_type_mismatch() {
    // Trying to access nested.value when nested is not an object
    let attributes = hashmap! {
        "nested".to_string() => AttributeValue::Int(42),
    };

    let value = resolve_path(&attributes, &["nested", "value"]);
    assert!(value.is_none());  // Can't traverse int
}
```

**Acceptance Criteria**:
- [ ] All attribute resolution edge cases handled
- [ ] Clear error messages

---

#### Task 4.2: Action/ActionSet Management (2 days)
**Priority**: 🟡 Important

Complete action and action set CRUD operations in graph.

```rust
// File: crates/arbor-graph-core/src/mutations.rs

impl Graph {
    pub fn upsert_action(&mut self, action: Action) -> Result<()> {
        // Validate action
        self.validate_action(&action)?;

        // Add to action storage
        self.actions.insert(action.id, action);

        Ok(())
    }

    pub fn remove_action(&mut self, action_id: &ActionId) -> Result<()> {
        // Check if action is used in any policies
        for policy in self.get_all_policies() {
            if policy.actions.contains(action_id) {
                return Err(Error::ActionInUse {
                    action_id: *action_id,
                    policy_id: policy.id,
                });
            }
        }

        self.actions.remove(action_id);
        Ok(())
    }

    pub fn upsert_action_set(&mut self, action_set: ActionSet) -> Result<()> {
        // Validate all actions exist
        for action_id in &action_set.actions {
            if !self.actions.contains_key(action_id) {
                return Err(Error::ActionNotFound(*action_id));
            }
        }

        self.action_sets.insert(action_set.id, action_set);
        Ok(())
    }

    pub fn remove_action_set(&mut self, action_set_id: &ActionSetId) -> Result<()> {
        // Check if action set is used in any policies
        for policy in self.get_all_policies() {
            if policy.action_sets.contains(action_set_id) {
                return Err(Error::ActionSetInUse {
                    action_set_id: *action_set_id,
                    policy_id: policy.id,
                });
            }
        }

        self.action_sets.remove(action_set_id);
        Ok(())
    }
}
```

**Implementation Steps**:
1. Implement `upsert_action()` with validation
2. Implement `remove_action()` with usage checks
3. Implement `upsert_action_set()` with validation
4. Implement `remove_action_set()` with usage checks
5. Add action → entity type associations (optional)

**Tests**:
```rust
#[test]
fn test_action_crud() {
    let mut graph = Graph::new();

    let action = Action {
        id: ActionId::from("read"),
        name: "Read".to_string(),
        entity_type: Some("document".into()),
    };

    graph.upsert_action(action.clone()).unwrap();
    assert!(graph.get_action(&action.id).is_some());

    graph.remove_action(&action.id).unwrap();
    assert!(graph.get_action(&action.id).is_none());
}

#[test]
fn test_cannot_remove_action_in_use() {
    let mut graph = Graph::new();

    let action = Action { /* ... */ };
    graph.upsert_action(action.clone()).unwrap();

    let policy = Policy {
        actions: vec![action.id],
        /* ... */
    };
    graph.upsert_policy(policy).unwrap();

    // Should fail - action is in use
    let result = graph.remove_action(&action.id);
    assert!(result.is_err());
}
```

**Acceptance Criteria**:
- [ ] All CRUD operations work
- [ ] Validation prevents invalid states
- [ ] Usage checks prevent dangling references

---

#### Task 4.3: Policy Split Helper (1 day)
**Priority**: 🟡 Important

Helper to split policies into 4 categories for authorization.

```rust
// File: crates/arbor-index-snapshot/src/lib.rs

pub fn split_policies(
    policies: Vec<&IndexedPolicy>,
) -> (
    Vec<&IndexedPolicy>,  // unconditional_forbid
    Vec<&IndexedPolicy>,  // conditional_forbid
    Vec<&IndexedPolicy>,  // unconditional_permit
    Vec<&IndexedPolicy>,  // conditional_permit
) {
    let mut unconditional_forbid = Vec::new();
    let mut conditional_forbid = Vec::new();
    let mut unconditional_permit = Vec::new();
    let mut conditional_permit = Vec::new();

    for policy in policies {
        let has_condition = policy.compiled_condition.is_some();

        match (policy.policy.policy_type, has_condition) {
            (PolicyType::Forbid, false) => unconditional_forbid.push(policy),
            (PolicyType::Forbid, true) => conditional_forbid.push(policy),
            (PolicyType::Permit, false) => unconditional_permit.push(policy),
            (PolicyType::Permit, true) => conditional_permit.push(policy),
        }
    }

    (unconditional_forbid, conditional_forbid, unconditional_permit, conditional_permit)
}
```

**Implementation Steps**:
1. Implement split logic
2. Add tests
3. Use in check() and list() operations

**Tests**:
```rust
#[test]
fn test_split_policies() {
    let policies = vec![
        create_policy(PolicyType::Forbid, false),   // Unconditional forbid
        create_policy(PolicyType::Forbid, true),    // Conditional forbid
        create_policy(PolicyType::Permit, false),   // Unconditional permit
        create_policy(PolicyType::Permit, true),    // Conditional permit
    ];

    let (uf, cf, up, cp) = split_policies(policies);

    assert_eq!(uf.len(), 1);
    assert_eq!(cf.len(), 1);
    assert_eq!(up.len(), 1);
    assert_eq!(cp.len(), 1);
}
```

**Acceptance Criteria**:
- [ ] Correctly categorizes all policies
- [ ] Used throughout authorization code

---

### Week 4-5 Deliverables

**Code**:
- ✅ Enhanced attribute resolution (~100 lines)
- ✅ Action/ActionSet CRUD (~300 lines)
- ✅ Policy split helper (~100 lines)
- ✅ Tests (~200 lines)

**Total**: ~700 lines

---

## Phase 1 Summary

### Total Deliverables

**Lines of Code**:
- Week 1-2: ~1,400 lines (Bytecode VM)
- Week 2-3: ~1,200 lines (Snapshot Builder)
- Week 3-4: ~1,500 lines (Authorization Operations)
- Week 4-5: ~700 lines (Helper Functions)
- **Total**: ~4,800 lines

**Key Components**:
1. ✅ Bytecode VM with full OpCode support
2. ✅ Snapshot builder with transitive closures
3. ✅ All 4 authorization operations
4. ✅ Policy validation at index time
5. ✅ Fail-closed semantics for Unknown/Invalid
6. ✅ Comprehensive test coverage

### Testing Strategy

**Unit Tests** (~1,500 lines):
- VM OpCode tests
- Compiler tests
- Closure computation tests
- Authorization logic tests

**Integration Tests** (~500 lines):
- End-to-end authorization flows
- Complex policy scenarios
- Edge cases

**Property-Based Tests** (~200 lines):
- Bytecode ≡ AST evaluation
- Policy correctness properties
- Snapshot consistency

### Performance Targets

- [x] check() p99 latency: <1ms
- [x] list_resources() p99 latency: <10ms (10K resources)
- [x] Bytecode VM 2-4x faster than AST
- [x] Snapshot generation: <200ms (10K entities, 1K policies)

### Exit Criteria

Before moving to Phase 2, verify:

- [ ] All unit tests passing
- [ ] All integration tests passing
- [ ] Performance benchmarks meet targets
- [ ] Code reviewed and documented
- [ ] No known critical bugs
- [ ] Memory usage acceptable (< 1GB for 100K entities)

### Risks & Mitigation

**Risk**: Bytecode VM slower than expected
- **Mitigation**: Benchmarked early (Week 1-2), fallback to AST if needed

**Risk**: Transitive closure computation too slow
- **Mitigation**: BFS with memoization, RoaringBitmaps for compression

**Risk**: Conditional policy evaluation bottleneck
- **Mitigation**: Two-phase strategy (bitmap first, then conditional)

---

## Next Steps

After Phase 1 completion:
1. **Week 5-6**: Build indexer service
2. **Week 6-7**: Build authorizer service with dual transport
3. **Week 7-8**: Create client libraries
4. **Week 8**: Implement broker connectors

See [Implementation Roadmap](./implementation-roadmap.md) for full plan.

---

**Document Status**: ✅ Ready for Implementation
**Last Updated**: 2026-02-26
**Owner**: Engineering Team
