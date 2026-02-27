# Implementation Plan: Phase 1 - Core Authorization Engine (Leveraging YSNP)

**Duration**: 2-4 weeks (reduced from 4-6 weeks by leveraging YSNP code)
**Goal**: Build the core authorization engine with bytecode VM, snapshot generation, and all authorization operations
**Status**: 🚧 Ready to Start

---

## 🎯 YSNP Code Availability Summary

The YSNP proof-of-concept at `../ysnp` contains working implementations of many Phase 1 components:

### ✅ Can Copy/Adapt from YSNP:
- **Condition Evaluator** (AST-based) - `ysnp-core/src/engine/condition_evaluator.rs` (~440 lines)
  - All operators implemented (And, Or, Not, Eq, Neq, Lt, Lte, Gt, Gte)
  - Set operations (In, Contains, ContainsAll, ContainsAny)
  - HasAttribute
  - Proper Unknown/Invalid propagation
  - Variable resolution with nested paths

- **Transitive Closure Computation** - `ysnp-core/src/store/core.rs` (lines 146-171)
  - Stack-based DFS for ancestors
  - Stack-based DFS for descendants
  - Already uses RoaringBitmaps

- **Check() Operation** - `ysnp-core/src/engine/check.rs` (~94 lines)
  - Bitmap-based policy filtering
  - 4-phase split (unconditional forbid, conditional forbid, unconditional permit, conditional permit)
  - Short-circuit logic

- **List Operations** - `ysnp-core/src/engine/listing.rs` (~180 lines)
  - list_resources() with two-phase strategy
  - list_principals() (inverted logic)
  - Bitmap operations + conditional evaluation

- **Index Store Structure** - `ysnp-core/src/store/index_store/core.rs`
  - UUID ↔ index mappings
  - Entity type indexes
  - Action → policy indexes
  - Ancestor/descendant bitmaps
  - Policy target types

### ⚠️ Needs Minor Modification:
- **Graph Structure** - YSNP uses NodeType enum, Arbor separates entities/policies/actions
- **Snapshot Format** - YSNP has slightly different structure, easy to adapt
- **Error Types** - Different error enums (but same patterns)

### ❌ Need to Build from Scratch:
- **Bytecode VM** - YSNP only has AST evaluation (commented out bytecode stub)
- **Bytecode Compiler** - Not present in YSNP
- **Batching Strategy** - Not present in YSNP
- **Broker Abstraction** - Not present in YSNP

---

## Week 1: Adapt YSNP Components (~5 days instead of 2 weeks)

### Task 1.1: Port Condition Evaluator from YSNP (1 day)
**Priority**: 🔴 Critical
**YSNP Source**: `ysnp-core/src/engine/condition_evaluator.rs`

Instead of building from scratch, **adapt** the working YSNP condition evaluator:

```bash
# Copy and adapt
cp ../ysnp/ysnp-core/src/engine/condition_evaluator.rs \
   crates/arbor-bytecode/src/ast_evaluator.rs
```

**Modifications Needed**:
1. Update imports (use arbor types instead of ysnp types)
2. Change `EvaluationContext` structure to match Arbor's
3. Replace interned string IDs with StringId<T>
4. Update error types to match arbor-types
5. Keep all the condition evaluation logic (it's excellent!)

**Why Keep AST Evaluator?**
- Use as fallback if bytecode VM has issues
- Use for property-based testing (bytecode ≡ AST)
- ~440 lines of battle-tested code

**Tests**:
- Port YSNP's condition tests
- Add new tests for Arbor-specific types

**Acceptance Criteria**:
- [ ] All condition types evaluate correctly
- [ ] Unknown/Invalid propagation works
- [ ] Nested attribute resolution works
- [ ] Tests pass

---

### Task 1.2: Port Transitive Closure from YSNP (0.5 days)
**Priority**: 🔴 Critical
**YSNP Source**: `ysnp-core/src/store/core.rs` (lines 146-171)

```bash
# Copy the closure computation
```

**Modifications Needed**:
1. Update to use Arbor's Graph structure
2. Change from HashSet to RoaringBitmap storage
3. Add cycle detection (safety check)
4. Update function signatures

**Implementation**:
```rust
// File: crates/arbor-indexer/src/closures.rs

// ADAPTED FROM YSNP (ysnp-core/src/store/core.rs:146-171)
pub fn compute_ancestors(
    graph: &Graph,
    entity_uuid: Uuid,
    parents_map: &HashMap<Uuid, Vec<Uuid>>,
) -> Result<RoaringBitmap> {
    let mut ancestors = RoaringBitmap::new();
    let mut seen = HashSet::new();
    let mut stack = Vec::new();

    // Get direct parents
    if let Some(parents) = parents_map.get(&entity_uuid) {
        stack.extend(parents.iter());
    }

    // Stack-based DFS (from YSNP)
    while let Some(parent_uuid) = stack.pop() {
        if seen.insert(*parent_uuid) {
            if let Some(&index) = graph.uuid_to_index.get(parent_uuid) {
                ancestors.insert(index as u32);

                // Add grandparents
                if let Some(grandparents) = parents_map.get(parent_uuid) {
                    stack.extend(grandparents.iter());
                }
            }
        }
    }

    Ok(ancestors)
}

pub fn compute_descendants(
    graph: &Graph,
    entity_uuid: Uuid,
    children_map: &HashMap<Uuid, Vec<Uuid>>,
) -> Result<RoaringBitmap> {
    // Same logic as ancestors but follow children
    // (from YSNP lines 160-171)
    // ...
}
```

**Acceptance Criteria**:
- [ ] Ancestors computed correctly
- [ ] Descendants computed correctly
- [ ] Cycle detection works
- [ ] Tests pass (port from YSNP)

---

### Task 1.3: Port Check() Implementation from YSNP (1 day)
**Priority**: 🔴 Critical
**YSNP Source**: `ysnp-core/src/engine/check.rs`

**Modifications Needed**:
1. Update to use Arbor's Snapshot structure
2. Integrate condition evaluator
3. Add fail-closed logic (YSNP has this partially)
4. Add reason tracking
5. Update error types

**Implementation**:
```rust
// File: services/arbor-authorizer/src/check.rs

// ADAPTED FROM YSNP (ysnp-core/src/engine/check.rs)
pub fn check(
    snapshot: &Snapshot,
    request: &CheckRequest,
) -> Result<CheckResponse> {
    // Get applicable policies (bitmap operations - from YSNP lines 18-26)
    let principal_bit = snapshot.uuid_to_index[&request.principal_id];
    let action_bit = snapshot.uuid_to_index[&request.action_id];
    let resource_bit = snapshot.uuid_to_index[&request.resource_id];

    let principal_policies = get_policies_for_principal(snapshot, principal_bit)?;
    let action_policies = get_policies_for_action(snapshot, action_bit)?;
    let resource_policies = get_policies_for_resource(snapshot, resource_bit)?;

    let applicable_policies = principal_policies & action_policies & resource_policies;

    // Split into 4 categories (from YSNP lines 28-33)
    let (
        unconditional_forbid,
        conditional_forbid,
        unconditional_permit,
        conditional_permit,
    ) = split_policy_map_for_authorization(snapshot, &applicable_policies);

    // 4-phase evaluation (from YSNP lines 35-91, enhanced with fail-closed)
    // Phase 1: Unconditional forbids
    if !unconditional_forbid.is_empty() {
        return Ok(CheckResponse::deny(/* reason */));
    }

    // Phase 2: Conditional forbids (ADD fail-closed logic)
    for policy in conditional_forbid {
        let result = evaluate_policy(policy, &context)?;
        match result {
            EvaluationResult::True | EvaluationResult::Unknown | EvaluationResult::Invalid => {
                // FAIL CLOSED (this is new vs YSNP)
                return Ok(CheckResponse::deny(/* reason */));
            }
            EvaluationResult::False => continue,
        }
    }

    // Phase 3 & 4: Permits (from YSNP)
    // ...
}
```

**Acceptance Criteria**:
- [ ] Check() returns correct decisions
- [ ] Forbid precedence works
- [ ] Fail-closed logic works
- [ ] Tests pass (adapt from YSNP + add new)

---

### Task 1.4: Port List Operations from YSNP (1 day)
**Priority**: 🔴 Critical
**YSNP Source**: `ysnp-core/src/engine/listing.rs`

**Modifications Needed**:
1. Update to use Arbor's Snapshot structure
2. Integrate condition evaluator
3. Add pagination support
4. Update error types

**Implementation**:
```rust
// File: services/arbor-authorizer/src/list.rs

// ADAPTED FROM YSNP (ysnp-core/src/engine/listing.rs)
pub fn list_resources(
    snapshot: &Snapshot,
    request: &ListResourcesRequest,
) -> Result<ListResourcesResponse> {
    // Phase 1: Bitmap operations (from YSNP lines 23-50)
    let principal_bit = snapshot.uuid_to_index[&request.principal_id];
    let principal_policies = get_policies_for_principal(snapshot, principal_bit)?;
    let action_policies = get_policies_for_action(snapshot, request.action_id)?;
    let applicable_policies = &principal_policies & &action_policies;

    let permitted_targets = find_permitted_targets(
        snapshot,
        &applicable_policies,
        request.resource_type,
    )?;

    // Phase 2: Conditional evaluation (from YSNP lines 35-45)
    // (YSNP has this working!)

    // Convert to UUIDs (from YSNP lines 61-65)
    let resources = permitted_targets
        .iter()
        .map(|bit| snapshot.index_to_uuid[bit as usize])
        .collect();

    Ok(ListResourcesResponse { resources, /* ... */ })
}
```

**Acceptance Criteria**:
- [ ] list_resources() works correctly
- [ ] list_principals() works correctly
- [ ] Forbids are respected
- [ ] Tests pass (adapt from YSNP)

---

### Task 1.5: Build Bytecode VM (2-3 days)
**Priority**: 🔴 Critical
**YSNP Source**: None (needs to be built)

**Why Not in YSNP?**
- YSNP only has AST evaluation
- Bytecode was planned but not implemented (stub exists in bytecode.rs)

This task follows the original plan from `implementation-plan-phase1.md` Task 1.1-1.6, but we can:
1. **Test against YSNP's AST evaluator** - Property-based tests (bytecode ≡ AST)
2. **Reuse test cases** from YSNP's condition tests
3. **Copy OpCode definitions** from YSNP's stub (if useful)

**Estimated Time**: 2-3 days (same as original, but with better test coverage from YSNP)

See original plan for detailed implementation steps.

**Acceptance Criteria**:
- [ ] Bytecode VM works correctly
- [ ] Property tests pass: `evaluate_ast(cond) == execute_bytecode(compile(cond))`
- [ ] 2-4x faster than AST evaluation
- [ ] All OpCodes implemented

---

## Week 2: Snapshot Builder (2-3 days instead of 1 week)

### Task 2.1: Build Snapshot Builder with YSNP Logic (2-3 days)
**Priority**: 🔴 Critical
**YSNP Source**: `ysnp-core/src/store/core.rs` (snapshot() function)

**Adapt from YSNP**:
1. UUID ↔ index mapping (lines 37-44, 59, 135, 138)
2. Entity type indexes (lines 44-48)
3. Policy target indexing (lines 60-109)
4. Action indexes (lines 117-132)
5. Transitive closure computation (lines 146-171)

**Add to YSNP Logic**:
1. Bytecode compilation (not in YSNP)
2. Policy validation with skip logic (not in YSNP)
3. Checksum computation (not in YSNP)
4. Batching trigger (not in YSNP)

**Implementation**:
```rust
// File: crates/arbor-indexer/src/snapshot_builder.rs

pub fn build_snapshot(&self) -> Result<Snapshot> {
    // PHASE 1: UUID ↔ Index mappings (from YSNP lines 37-144)
    let mut uuid_to_index = HashMap::new();
    let mut index_to_uuid = Vec::new();
    // ... (copy YSNP logic)

    // PHASE 2: Build parent/child maps (from YSNP lines 34-56)
    let mut parents = HashMap::new();
    let mut children = HashMap::new();
    // ... (copy YSNP logic)

    // PHASE 3: Compute closures (from YSNP lines 146-171)
    let indexed_entities = self.build_indexed_entities_with_closures(
        &uuid_to_index,
        &parents,
        &children,
    )?;

    // PHASE 4: Compile policies (NEW - not in YSNP)
    let indexed_policies = self.compile_and_index_policies(&uuid_to_index)?;

    // PHASE 5: Build specialized indexes (from YSNP lines 44-132)
    let entity_type_indexes = self.build_entity_type_indexes(&indexed_entities);
    let action_policy_indexes = self.build_action_indexes(&indexed_policies);

    // PHASE 6: Compute checksum (NEW - not in YSNP)
    let checksum = self.compute_checksum(&indexed_entities, &indexed_policies)?;

    Ok(Snapshot { /* ... */ })
}
```

**Acceptance Criteria**:
- [ ] Snapshot generation works
- [ ] All indexes built correctly
- [ ] Closures computed correctly
- [ ] Policies compiled to bytecode
- [ ] Tests pass

---

### Task 2.2: Add Batching Strategy (0.5 days)
**Priority**: 🟡 Important
**YSNP Source**: None (not needed in proof-of-concept)

Follow original plan Task 2.4 from `implementation-plan-phase1.md`.

**Acceptance Criteria**:
- [ ] Batching strategies work
- [ ] Configuration flexible
- [ ] Tests pass

---

## Week 2-3: Helper Functions (1 day)

### Task 3.1: Attribute Resolution (Already Done!)
**YSNP Source**: `ysnp-core/src/engine/condition_evaluator.rs` (lines 297-348)

YSNP's `resolve_operand()` and nested attribute resolution is excellent and already handles:
- Simple attributes
- Nested paths (e.g., `principal.profile.level`)
- Missing attributes → Unknown
- Type conversions

**Action**: Port tests, add edge cases

---

### Task 3.2: Action/ActionSet Management (0.5 days)
**Priority**: 🟡 Important
**YSNP Source**: Partially in `ysnp-core/src/store/core.rs` (lines 122-132)

YSNP shows how to expand action sets during indexing. We need to add:
- CRUD operations in graph
- Usage validation

Follow original plan Task 4.2.

---

### Task 3.3: Policy Split Helper (0.5 days)
**Priority**: 🟡 Important
**YSNP Source**: Referenced in check.rs but implementation not shown

YSNP splits policies into 4 categories. We need the helper function.

Follow original plan Task 4.3.

---

## Revised Timeline

### Week 1 (5 days)
- ✅ Day 1: Port condition evaluator from YSNP
- ✅ Day 2: Port closures + check() from YSNP
- ✅ Day 3: Port list operations from YSNP
- 🔨 Day 4-5: Build bytecode VM (new, test against YSNP AST)

### Week 2 (3 days)
- 🔨 Day 1-2: Build snapshot builder (adapt YSNP snapshot logic + add bytecode compilation)
- ✅ Day 3: Add batching + helper functions

### Week 3 (Optional - Testing & Polish)
- Integration tests
- Performance benchmarks
- Property-based tests (bytecode ≡ AST)
- Documentation

---

## Key Changes from Original Plan

### Time Savings
- **Original**: 4-6 weeks
- **With YSNP**: 2-4 weeks (50% reduction!)

### What We Get from YSNP
1. ✅ **Condition Evaluator** - 440 lines, battle-tested
2. ✅ **Transitive Closure** - Working algorithm
3. ✅ **Check() Logic** - 4-phase evaluation
4. ✅ **List Operations** - Two-phase strategy
5. ✅ **Index Structure** - Proven approach
6. ✅ **Test Cases** - Real-world scenarios
7. ✅ **String Interning** - Identical implementation, copy directly!

### What We Still Build
1. 🔨 **Bytecode VM** - 2-4x performance improvement
2. 🔨 **Bytecode Compiler** - AST → Bytecode
3. 🔨 **Batching Strategy** - Snapshot generation control
4. 🔨 **Policy Validation** - Three-stage validation
5. 🔨 **Checksum Logic** - Snapshot integrity

### Testing Strategy
- **Port YSNP tests** for condition evaluation, closures, check(), list()
- **Add property-based tests** to verify bytecode ≡ AST
- **Benchmark** bytecode VM vs AST (target: 2-4x faster)
- **Integration tests** for end-to-end flows

---

## File Mapping: YSNP → Arbor

| YSNP File | Arbor File | Action |
|-----------|------------|--------|
| `engine/condition_evaluator.rs` | `arbor-bytecode/src/ast_evaluator.rs` | Copy + adapt |
| `store/core.rs` (lines 146-171) | `arbor-indexer/src/closures.rs` | Copy + adapt |
| `engine/check.rs` | `arbor-authorizer/src/check.rs` | Copy + enhance |
| `engine/listing.rs` | `arbor-authorizer/src/list.rs` | Copy + adapt |
| `store/index_store/core.rs` | `arbor-index-snapshot/src/lib.rs` | Reference for structure |
| `types/` | `arbor-types/src/` | Already ported (Phase 0) |

---

## Migration Notes

### Type Conversions

**Good News**: YSNP and Arbor use **identical string interning approaches**!

```rust
// YSNP
type EntityTypeId = StringId<EntityTypeMarker>;  // u32 wrapper

// Arbor
type EntityTypeId = StringId<EntityTypeMarker>;  // u32 wrapper (same!)
```

Both use:
- `StringId<T>` wrapping `u32`
- `StringInterner<T>` with HashMap-based interning
- PhantomData for type safety

**Porting is trivial**:
- API is identical: `interner.intern(string)`, `interner.get_id(string)`
- Bitmap indexes work directly (both use `u32`)
- No type conversions needed!

### Error Handling
```rust
// YSNP
return Err(YsnpError::EntityNotFound(uuid));

// Arbor
return Err(Error::EntityNotFound { entity_id: uuid });
```

### Context Structure
```rust
// YSNP
struct EvaluationContext<'a> {
    principal: Option<&'a Entity>,
    resource: Option<&'a Entity>,
    action: &'a Action,
    context: Option<&'a Attributes>,
    graph_oracle: &'a dyn GraphOracle,
}

// Arbor (simplified, no graph oracle in snapshot)
struct EvaluationContext<'a> {
    principal: &'a IndexedEntity,
    resource: &'a IndexedEntity,
    context_attributes: &'a Attributes,
    snapshot: &'a Snapshot,
}
```

---

## Success Metrics (Unchanged)

- [ ] check() p99 latency: <1ms
- [ ] list_resources() p99 latency: <10ms (10K resources)
- [ ] Bytecode VM 2-4x faster than AST
- [ ] Snapshot generation: <200ms (10K entities, 1K policies)
- [ ] 100% test coverage for core logic
- [ ] Property tests pass (bytecode ≡ AST)

---

## Risk Mitigation

**Risk**: YSNP code doesn't fit Arbor architecture
- **Mitigation**: Keep YSNP code as reference, rewrite if needed
- **Likelihood**: Low (architectures are similar)

**Risk**: Type conversions introduce bugs
- **Mitigation**: Comprehensive test coverage, port YSNP's tests first

**Risk**: Bytecode VM still takes 2-3 days
- **Mitigation**: This is expected, not a risk. We still have AST fallback from YSNP.

---

**Document Status**: ✅ Ready for Implementation
**Last Updated**: 2026-02-26
**Estimated Time Savings**: 2-3 weeks by leveraging YSNP
**Next Steps**: Start with Task 1.1 (Port Condition Evaluator)
