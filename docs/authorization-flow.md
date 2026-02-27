# Authorization Flow

This document describes how Arbor processes authorization requests and list queries.

## Overview

Arbor supports four core authorization operations:

1. **`check()`** - "Can principal X perform action Y on resource Z?"
2. **`list_resources()`** - "What resources can principal X perform action Y on?"
3. **`list_principals()`** - "What principals can perform action Y on resource Z?"
4. **`list_actions()`** - "What actions can principal X perform on resource Z?"

All operations support:
- **Context attributes**: Additional data for condition evaluation (time, IP, custom fields)
- **Optional explanation**: Return reason for decision (debug mode)
- **Hierarchical evaluation**: Considers entity ancestors/descendants
- **Type-based targeting**: Policies can target all entities of a type

## Request Format

### Check Request

```rust
struct CheckRequest {
    principal_id: Uuid,
    action_id: Uuid,
    resource_id: Uuid,
    context: Attributes,      // Optional context data
    explain: bool,            // Return explanation? (default: false)
}

struct CheckResponse {
    decision: Decision,       // Permit, Deny
    reason: Option<Reason>,   // Only if explain=true
    snapshot_version: u64,    // Which snapshot was used
}

enum Decision {
    Permit,
    Deny,
}

enum Reason {
    NoApplicablePolicies,
    ForbiddenBy { policy_id: Uuid, policy_name: String },
    PermittedBy { policy_id: Uuid, policy_name: String },
    ConditionFailed { policy_id: Uuid, condition: String },
}
```

### List Resources Request

```rust
struct ListResourcesRequest {
    principal_id: Uuid,
    action_id: Uuid,
    resource_type: EntityTypeId,  // Optional: filter by type
    context: Attributes,
    limit: Option<u32>,           // Pagination
    offset: Option<u32>,
}

struct ListResourcesResponse {
    resources: Vec<Entity>,
    total_count: u32,
    snapshot_version: u64,
}
```

### List Principals Request

```rust
struct ListPrincipalsRequest {
    action_id: Uuid,
    resource_id: Uuid,
    principal_type: EntityTypeId, // Optional: filter by type
    context: Attributes,
    limit: Option<u32>,
    offset: Option<u32>,
}

struct ListPrincipalsResponse {
    principals: Vec<Entity>,
    total_count: u32,
    snapshot_version: u64,
}
```

### List Actions Request

```rust
struct ListActionsRequest {
    principal_id: Uuid,
    resource_id: Uuid,
    context: Attributes,
}

struct ListActionsResponse {
    actions: Vec<Action>,
    snapshot_version: u64,
}
```

## Check Operation

### High-Level Algorithm

```
1. Get applicable policies
2. Split into 4 categories (unconditional forbid, conditional forbid, unconditional permit, conditional permit)
3. Evaluate in order with short-circuiting
4. Return decision + reason (if explain=true)
```

### Detailed Flow

#### Step 1: Get Applicable Policies

Get policies that could apply to this request:

```rust
// Get policies where this entity is the principal (or ancestor/type)
let principal_policies = snapshot.get_policies_for_principal(principal_id);

// Get policies where this entity is the resource (or ancestor/type)
let resource_policies = snapshot.get_policies_for_resource(resource_id);

// Get policies for this action (including action sets)
let action_policies = snapshot.get_policies_for_action(action_id);

// Intersect: policies must match principal AND resource AND action
let applicable_policies = principal_policies
    .intersection(&resource_policies)
    .intersection(&action_policies);
```

**Principal Policy Matching**:
- Direct: Policy targets this specific principal UUID
- Ancestral: Policy targets an ancestor with `PolicyTarget::EntityWithDescendants`
- Type-based: Policy targets all entities of this principal's type
- All: Policy targets `PolicyTarget::All`

**Resource Policy Matching**: Same logic as principal

**Action Policy Matching**:
- Direct: Policy specifies this action UUID
- ActionSet: Policy specifies an action set that contains this action

#### Step 2: Split Policies into Categories

```rust
let (
    unconditional_forbid,
    conditional_forbid,
    unconditional_permit,
    conditional_permit
) = snapshot.split_policy_map_for_authorization(&applicable_policies);
```

**Unconditional**: Policies with no condition (always apply)
**Conditional**: Policies with a condition that must be evaluated

**Forbid**: `PolicyType::Forbid` (deny access)
**Permit**: `PolicyType::Permit` (grant access)

#### Step 3: Evaluate with Short-Circuiting

```rust
// 1. Check unconditional forbids
if !unconditional_forbid.is_empty() {
    let policy = unconditional_forbid[0]; // Pick first for explanation
    return CheckResponse {
        decision: Decision::Deny,
        reason: Some(Reason::ForbiddenBy {
            policy_id: policy.id,
            policy_name: policy.name,
        }),
        snapshot_version,
    };
}

// 2. Evaluate conditional forbids
for policy in conditional_forbid {
    let result = evaluate_bytecode(
        policy.compiled_condition,
        principal_attributes,
        resource_attributes,
        context_attributes
    );

    // Fail closed: Unknown/Invalid treated as True for forbids
    if result == ConditionResult::True
        || result == ConditionResult::Unknown
        || result == ConditionResult::Invalid
    {
        return CheckResponse {
            decision: Decision::Deny,
            reason: Some(Reason::ForbiddenBy {
                policy_id: policy.id,
                policy_name: policy.name,
            }),
            snapshot_version,
        };
    }
}

// 3. Check unconditional permits
if !unconditional_permit.is_empty() {
    let policy = unconditional_permit[0];
    return CheckResponse {
        decision: Decision::Permit,
        reason: Some(Reason::PermittedBy {
            policy_id: policy.id,
            policy_name: policy.name,
        }),
        snapshot_version,
    };
}

// 4. Evaluate conditional permits
for policy in conditional_permit {
    let result = evaluate_bytecode(
        policy.compiled_condition,
        principal_attributes,
        resource_attributes,
        context_attributes
    );

    // Only permit if condition is definitely True (not Unknown/Invalid)
    if result == ConditionResult::True {
        return CheckResponse {
            decision: Decision::Permit,
            reason: Some(Reason::PermittedBy {
                policy_id: policy.id,
                policy_name: policy.name,
            }),
            snapshot_version,
        };
    }

    // Unknown/Invalid: Don't apply the permit (continue to default deny)
}

// 5. Default deny
return CheckResponse {
    decision: Decision::Deny,
    reason: Some(Reason::NoApplicablePolicies),
    snapshot_version,
};
```

### Evaluation Semantics

**Forbid Precedence**: A single forbid (unconditional or conditional that evaluates to true) **overrides any permits**.

**Default Deny**: If no permits apply, access is denied.

**Evaluation Order**:
1. Unconditional forbids (fastest short-circuit)
2. Conditional forbids (evaluate until one is true)
3. Unconditional permits (fast path if no conditional forbids)
4. Conditional permits (evaluate until one is true)
5. Default deny

**Short-Circuit Strategy**:
- Stop at first forbid that applies (don't evaluate remaining policies)
- Return the reason (which policy caused the denial/permit)
- Performance: Most requests short-circuit early (unconditional policies)

### Performance Characteristics

**Fast Path** (no conditions):
- Bitmap intersection: O(n) in compressed bitmap size
- Typically <1 microsecond for small policy sets

**Slow Path** (with conditions):
- Bytecode evaluation: ~100-500 nanoseconds per condition
- Dominated by attribute lookup, not VM execution
- Still submillisecond for typical policies

**Worst Case**:
- Many conditional policies that all evaluate to false
- Must evaluate all forbids, then all permits
- Still single-digit milliseconds for reasonable policy counts

## List Resources Operation

**Goal**: "What resources can principal X perform action Y on?"

This is more complex than `check()` because we need to find **all** resources that pass the authorization check.

### Two-Phase Strategy

#### Phase 1: Bitmap Operations (Unconditional Policies)

```rust
// Get all policies applicable to this principal and action
let principal_policies = snapshot.get_policies_for_principal(principal_id);
let action_policies = snapshot.get_policies_for_action(action_id);
let applicable = principal_policies.intersection(&action_policies);

// Split by conditionality and type
let (uncond_forbid, cond_forbid, uncond_permit, cond_permit) =
    snapshot.split_policy_map_for_authorization(&applicable);

// Get resources for unconditional permits
let mut permitted_resources = RoaringBitmap::new();
for policy_idx in uncond_permit.iter() {
    let policy = &snapshot.indexed_policies[policy_idx];

    match policy.resource_target {
        PolicyTarget::Entity(uuid) => {
            let idx = snapshot.uuid_to_index[&uuid];
            permitted_resources.insert(idx);
        }
        PolicyTarget::EntityWithDescendants(uuid) => {
            let idx = snapshot.uuid_to_index[&uuid];
            permitted_resources.insert(idx);
            permitted_resources.or(&snapshot.indexed_entities[idx].descendants);
        }
        PolicyTarget::EntityType(type_id) => {
            permitted_resources.or(&snapshot.entity_type_to_indices[&type_id]);
        }
        PolicyTarget::All => {
            permitted_resources.or(&snapshot.all_entity_indices);
        }
    }
}

// Get resources for unconditional forbids
let mut forbidden_resources = RoaringBitmap::new();
for policy_idx in uncond_forbid.iter() {
    let policy = &snapshot.indexed_policies[policy_idx];
    // Same logic as permits
    // ...
}

// Fast path: unconditional permits minus unconditional forbids
permitted_resources.andnot(&forbidden_resources);
```

**Result**: Bitmap of resource indices that are permitted by unconditional policies.

#### Phase 2: Conditional Evaluation (Residuals)

For resources with conditional policies, we must evaluate each condition:

```rust
// Resources that have conditional policies
let conditional_resources = /* resources with cond_forbid or cond_permit */;

for resource_idx in conditional_resources.iter() {
    let resource_id = snapshot.index_to_uuid[resource_idx];

    // Run check() for this specific resource
    let result = check(principal_id, action_id, resource_id, context, false);

    if result.decision == Decision::Permit {
        permitted_resources.insert(resource_idx);
    }
}

// Convert bitmap to Entity list
let resources: Vec<Entity> = permitted_resources
    .iter()
    .map(|idx| snapshot.indexed_entities[idx].clone())
    .collect();
```

### Optimization: Attribute Shape Caching

Many resources have the same attribute structure. We can cache condition evaluation results:

```rust
struct AttributeShapeKey {
    attribute_names: BTreeSet<AttributeNameId>,
    // Could include value hashes for more precise caching
}

let mut condition_cache: HashMap<(PolicyId, AttributeShapeKey), ConditionResult> = HashMap::new();

for resource_idx in conditional_resources.iter() {
    let shape_key = compute_attribute_shape(&snapshot.indexed_entities[resource_idx]);

    for policy_idx in conditional_policies.iter() {
        let cache_key = (policy_idx, shape_key);

        let result = condition_cache.entry(cache_key).or_insert_with(|| {
            // Evaluate condition only once per unique attribute shape
            evaluate_bytecode(...)
        });

        // Use cached result
    }
}
```

**Performance**: If you have 10,000 files but only 3 attribute shapes (public, private, restricted), you evaluate conditions 3 times instead of 10,000 times.

### Performance Characteristics

**Best Case** (all unconditional):
- Pure bitmap operations: <1 millisecond for 100K resources

**Typical Case** (mostly unconditional):
- Bitmap operations + sparse conditional evaluation
- 1-5 milliseconds for 10K resources

**Worst Case** (all conditional):
- Must evaluate condition for every resource
- With caching: 5-20 milliseconds for 10K resources
- Without caching: 50-500 milliseconds for 10K resources

## List Principals Operation

**Goal**: "What principals can perform action Y on resource Z?"

This is symmetric to `list_resources()`:

### Algorithm

```rust
// Phase 1: Bitmap operations for unconditional policies
let resource_policies = snapshot.get_policies_for_resource(resource_id);
let action_policies = snapshot.get_policies_for_action(action_id);
let applicable = resource_policies.intersection(&action_policies);

// Get permitted principals from unconditional policies
let mut permitted_principals = RoaringBitmap::new();
for policy_idx in unconditional_permit.iter() {
    match policy.principal_target {
        PolicyTarget::Entity(uuid) => { /* add to bitmap */ }
        PolicyTarget::EntityWithDescendants(uuid) => { /* add entity + descendants */ }
        PolicyTarget::EntityType(type_id) => { /* add all of type */ }
        PolicyTarget::All => { /* add all */ }
    }
}

// Phase 2: Conditional evaluation
for principal_idx in conditional_principals.iter() {
    let principal_id = snapshot.index_to_uuid[principal_idx];
    let result = check(principal_id, action_id, resource_id, context, false);
    if result.decision == Decision::Permit {
        permitted_principals.insert(principal_idx);
    }
}
```

Same optimization opportunities as `list_resources()`.

## List Actions Operation

**Goal**: "What actions can principal X perform on resource Z?"

This is typically faster because action sets are usually small.

### Algorithm

```rust
// Get all policies applicable to this principal and resource
let principal_policies = snapshot.get_policies_for_principal(principal_id);
let resource_policies = snapshot.get_policies_for_resource(resource_id);
let applicable = principal_policies.intersection(&resource_policies);

// For each policy, collect allowed actions
let mut permitted_actions = HashSet::new();
let mut forbidden_actions = HashSet::new();

for policy_idx in applicable.iter() {
    let policy = &snapshot.indexed_policies[policy_idx];

    // Check condition if present
    let applies = if let Some(condition) = &policy.compiled_condition {
        evaluate_bytecode(condition, ...) == ConditionResult::True
    } else {
        true
    };

    if applies {
        if policy.policy_type == PolicyType::Permit {
            permitted_actions.extend(&policy.actions);
            // Also expand action sets
            for action_set_id in &policy.action_sets {
                permitted_actions.extend(&snapshot.action_sets[action_set_id].actions);
            }
        } else {
            forbidden_actions.extend(&policy.actions);
            for action_set_id in &policy.action_sets {
                forbidden_actions.extend(&snapshot.action_sets[action_set_id].actions);
            }
        }
    }
}

// Forbids override permits
permitted_actions.retain(|action| !forbidden_actions.contains(action));

let actions: Vec<Action> = permitted_actions
    .iter()
    .map(|id| snapshot.actions[id].clone())
    .collect();
```

### Performance Characteristics

- Typically very fast (<1 millisecond)
- Action sets are small (usually <100 actions)
- Rare to have many conditional policies per principal-resource pair

## Condition Evaluation

See [Bytecode VM](./bytecode-vm.md) for details on how conditions are evaluated.

### Evaluation Context

```rust
struct EvaluationContext {
    principal_attributes: Attributes,
    resource_attributes: Attributes,
    context_attributes: Attributes,
    action: Action,
}
```

### Variable Resolution

Conditions can reference attributes via variable references:

```rust
// In condition: principal.profile.tier
VariableRef {
    scope: Scope::Principal,
    path: vec!["profile", "tier"]
}

// Resolution
fn resolve_attribute(var_ref: &VariableRef) -> Result<AttributeValue> {
    let base = match var_ref.scope {
        Scope::Principal => &self.principal_attributes,
        Scope::Resource => &self.resource_attributes,
        Scope::Context => &self.context_attributes,
    };

    // Walk nested path
    let mut current = base;
    for segment in &var_ref.path {
        current = current.get(segment)?;
    }
    Ok(current.clone())
}
```

### Condition Result

```rust
enum ConditionResult {
    True,       // Condition evaluates to true
    False,      // Condition evaluates to false
    Unknown,    // Missing data (e.g., attribute not present)
    Invalid,    // Error during evaluation (e.g., type mismatch)
}
```

**Handling Unknown/Invalid (Fail Closed)**:
- For **forbid** policies: Treat `Unknown`/`Invalid` as `True` (forbid if can't evaluate - fail closed)
- For **permit** policies: Treat `Unknown`/`Invalid` as `False` (don't permit if can't evaluate)
- **Security principle**: When uncertain, deny access. Better to incorrectly deny than incorrectly grant.

## Optimization Techniques

### 1. Bitmap Intersection

Use Roaring bitmaps for fast set operations:
```rust
// Instead of iterating all policies
for policy in all_policies {
    if matches_principal && matches_resource && matches_action { ... }
}

// Do bitmap intersection
let applicable = principal_policies
    .intersection(&resource_policies)
    .intersection(&action_policies);
```

**Performance**: O(n) in compressed bitmap size, typically <1 microsecond

### 2. Early Termination

Stop evaluating as soon as decision is known:
```rust
// Don't do this:
let has_forbid = conditional_forbids.iter().any(|p| evaluate(p));
let has_permit = conditional_permits.iter().any(|p| evaluate(p));
if has_forbid { deny() } else if has_permit { allow() }

// Do this:
for policy in conditional_forbids {
    if evaluate(policy) {
        return deny();  // Stop immediately
    }
}
```

### 3. Precomputed Indexes

Store multiple views of the same data:
```rust
// Instead of scanning all policies
policies.iter().filter(|p| p.principal == principal_id)

// Use precomputed index
snapshot.principal_to_policies[&principal_id]
```

### 4. Attribute Shape Caching

Cache condition evaluation results for resources with the same attribute structure (see List Resources section).

### 5. Bytecode Compilation

Compile conditions once, interpret many times (see [Bytecode VM](./bytecode-vm.md)).

## Error Handling

### Invalid Entity IDs

```rust
if !snapshot.uuid_to_index.contains_key(&principal_id) {
    return Err(AuthorizationError::PrincipalNotFound { id: principal_id });
}
```

### Invalid Action IDs

```rust
if !snapshot.actions.contains_key(&action_id) {
    return Err(AuthorizationError::ActionNotFound { id: action_id });
}
```

### Condition Evaluation Errors

```rust
match evaluate_bytecode(...) {
    Ok(result) => result,
    Err(e) => {
        // Log error but don't fail authorization request
        log::warn!("Condition evaluation error: {}", e);
        ConditionResult::Invalid
        // For forbid policies: Treat as True → Deny access (fail closed)
        // For permit policies: Treat as False → Don't grant access
    }
}
```

### Missing Attributes

```rust
// If condition references principal.profile.tier but attribute doesn't exist
// Return ConditionResult::Unknown
// For forbid policies: Treat as True → Apply forbid (fail closed)
// For permit policies: Treat as False → Don't apply permit
```

### Invalid Policies at Evaluation Time

**When**: During check() operation, a policy is found to be invalid (corrupted bytecode, broken references, etc.)

**Action**: Fail closed based on policy type

```rust
for policy_idx in conditional_forbid {
    let policy = &snapshot.indexed_policies[policy_idx];

    match evaluate_policy(policy, context) {
        Ok(result) => {
            // Normal evaluation
            if should_apply_forbid(result) {
                return Decision::Deny;
            }
        }
        Err(e) => {
            // INVALID POLICY at evaluation time
            log::error!(
                "Invalid forbid policy {} ({}): {}",
                policy.id,
                policy.name,
                e
            );

            metrics::counter!(
                "arbor.authorizer.invalid_policy",
                1,
                "policy_type" => "forbid",
                "error_type" => format!("{:?}", e)
            );

            // FAIL CLOSED: Apply forbid anyway
            return CheckResponse {
                decision: Decision::Deny,
                reason: Some(Reason::PolicyEvaluationError {
                    policy_id: policy.id,
                    error: format!("Policy evaluation failed: {}", e),
                }),
                snapshot_version,
            };
        }
    }
}

for policy_idx in conditional_permit {
    let policy = &snapshot.indexed_policies[policy_idx];

    match evaluate_policy(policy, context) {
        Ok(ConditionResult::True) => {
            return Decision::Permit;
        }
        Ok(_) => {
            continue;
        }
        Err(e) => {
            // INVALID POLICY at evaluation time
            log::error!(
                "Invalid permit policy {} ({}): {}",
                policy.id,
                policy.name,
                e
            );

            metrics::counter!(
                "arbor.authorizer.invalid_policy",
                1,
                "policy_type" => "permit",
                "error_type" => format!("{:?}", e)
            );

            // FAIL CLOSED: Don't grant access, continue to default deny
            continue;
        }
    }
}
```

**Security Rationale**:
- **Invalid forbid policy** → Treat as forbid (deny access) - safer than ignoring it
- **Invalid permit policy** → Don't permit (deny access) - don't grant on error
- Both cases lead to denial, which is secure

**When this happens**:
- Corrupted bytecode (should be caught by checksum, but defense in depth)
- Memory corruption (extremely rare)
- Future: Schema evolution issues

This should be **very rare** in production if checksums are working correctly.

## Observability

### Metrics

```rust
// Per-operation metrics
metrics::histogram!("arbor.check.duration_ms", duration);
metrics::counter!("arbor.check.permit", 1);
metrics::counter!("arbor.check.deny", 1);

// Policy evaluation metrics
metrics::histogram!("arbor.policy.evaluation_duration_us", duration);
metrics::counter!("arbor.policy.short_circuit", 1);

// List operation metrics
metrics::histogram!("arbor.list_resources.duration_ms", duration);
metrics::histogram!("arbor.list_resources.count", resources.len());
```

### Tracing

```rust
use tracing::{info_span, instrument};

#[instrument(skip(self, context))]
async fn check(
    &self,
    principal_id: Uuid,
    action_id: Uuid,
    resource_id: Uuid,
    context: Attributes,
    explain: bool,
) -> CheckResponse {
    let _span = info_span!("check",
        principal = %principal_id,
        action = %action_id,
        resource = %resource_id
    ).entered();

    // ... operation
}
```

### Logging

```rust
if explain {
    log::debug!(
        "Authorization decision: {:?} for principal={} action={} resource={} reason={:?}",
        response.decision,
        principal_id,
        action_id,
        resource_id,
        response.reason
    );
}
```

## Related Documentation

- [Policy Evaluation](./policy-evaluation.md) - Detailed evaluation semantics
- [Bytecode VM](./bytecode-vm.md) - How conditions are compiled and evaluated
- [Snapshot Format](./snapshot-format.md) - Index structure for fast lookups
- [Data Model](./data-model.md) - Entities, policies, actions, attributes
