# Policy Validation

This document describes how Arbor validates policies at different stages of the system.

## Overview

Policies are validated at **three stages**:

```
┌─────────────────┐    ┌──────────────┐    ┌──────────────┐
│ Write Time      │ → │ Index Time   │ → │ Eval Time    │
│ (Graph update)  │    │ (Snapshot)   │    │ (Check)      │
│                 │    │              │    │              │
│ REJECT invalid  │    │ SKIP invalid │    │ FAIL CLOSED  │
└─────────────────┘    └──────────────┘    └──────────────┘
```

Each stage has different validation rules and error handling strategies.

---

## Stage 1: Write Time Validation (Graph)

**When**: Policy is created or updated via `graph.upsert_policy()`

**Goal**: Prevent invalid policies from entering the system

### Validation Checks

```rust
pub fn upsert_policy(graph: &mut Graph, policy: Policy) -> Result<(), PolicyValidationError> {
    // 1. Structural validation
    validate_policy_structure(&policy)?;

    // 2. Entity reference validation
    validate_policy_targets(&graph, &policy)?;

    // 3. Action reference validation
    validate_policy_actions(&graph, &policy)?;

    // 4. Condition syntax validation
    validate_policy_conditions(&policy)?;

    // 5. Insert into graph
    graph.insert_policy(policy)?;

    Ok(())
}
```

### 1. Structural Validation

```rust
fn validate_policy_structure(policy: &Policy) -> Result<()> {
    // Check required fields
    if policy.name.is_empty() {
        return Err(PolicyValidationError::EmptyName);
    }

    if policy.actions.is_empty() && policy.action_sets.is_empty() {
        return Err(PolicyValidationError::NoActions {
            message: "Policy must specify at least one action or action set"
        });
    }

    // Check UUID validity
    if policy.id == Uuid::nil() {
        return Err(PolicyValidationError::InvalidId);
    }

    Ok(())
}
```

### 2. Entity Reference Validation

```rust
fn validate_policy_targets(graph: &Graph, policy: &Policy) -> Result<()> {
    // Validate principal target
    match &policy.principal {
        PolicyTarget::Entity(uuid) | PolicyTarget::EntityWithDescendants(uuid) => {
            if !graph.entity_exists(uuid) {
                return Err(PolicyValidationError::InvalidPrincipalTarget {
                    entity_id: *uuid,
                    message: "Referenced entity does not exist"
                });
            }
        }
        PolicyTarget::EntityType(type_id) => {
            // Entity types are string IDs, always valid
            // Could optionally validate against type registry
        }
        PolicyTarget::All => {
            // Always valid
        }
    }

    // Validate resource target (same logic)
    match &policy.resource {
        PolicyTarget::Entity(uuid) | PolicyTarget::EntityWithDescendants(uuid) => {
            if !graph.entity_exists(uuid) {
                return Err(PolicyValidationError::InvalidResourceTarget {
                    entity_id: *uuid,
                    message: "Referenced entity does not exist"
                });
            }
        }
        // ... etc
    }

    Ok(())
}
```

### 3. Action Reference Validation

```rust
fn validate_policy_actions(graph: &Graph, policy: &Policy) -> Result<()> {
    // Validate direct actions
    for action_id in &policy.actions {
        if !graph.action_exists(action_id) {
            return Err(PolicyValidationError::InvalidAction {
                action_id: *action_id,
                message: "Referenced action does not exist"
            });
        }
    }

    // Validate action sets
    for action_set_id in &policy.action_sets {
        if !graph.action_set_exists(action_set_id) {
            return Err(PolicyValidationError::InvalidActionSet {
                action_set_id: *action_set_id,
                message: "Referenced action set does not exist"
            });
        }
    }

    Ok(())
}
```

### 4. Condition Syntax Validation

```rust
fn validate_policy_conditions(policy: &Policy) -> Result<()> {
    if let Some(condition) = &policy.conditions {
        validate_condition(condition)?;
    }
    Ok(())
}

fn validate_condition(condition: &Condition) -> Result<()> {
    match condition {
        Condition::And(left, right) | Condition::Or(left, right) => {
            validate_condition(left)?;
            validate_condition(right)?;
        }
        Condition::Not(inner) => {
            validate_condition(inner)?;
        }
        Condition::Eq(left, right) | Condition::Neq(left, right) => {
            validate_comparable_types(left, right)?;
        }
        Condition::Lt(left, right) | Condition::Lte(left, right)
        | Condition::Gt(left, right) | Condition::Gte(left, right) => {
            validate_orderable_types(left, right)?;
        }
        Condition::In(element, set) => {
            validate_set_membership(element, set)?;
        }
        Condition::HasAttribute(var_ref) => {
            validate_variable_reference(var_ref)?;
        }
        // ... etc
    }
    Ok(())
}

fn validate_comparable_types(left: &ValueExpr, right: &ValueExpr) -> Result<()> {
    match (left, right) {
        (ValueExpr::Literal(l), ValueExpr::Literal(r)) => {
            if !can_compare(l, r) {
                return Err(PolicyValidationError::IncomparableTypes {
                    left: format!("{:?}", l),
                    right: format!("{:?}", r),
                });
            }
        }
        // Variables can't be type-checked at write time (runtime values)
        _ => {}
    }
    Ok(())
}
```

### Error Response

```rust
match graph.upsert_policy(policy) {
    Ok(()) => {
        // Policy accepted
        StatusCode::OK
    }
    Err(PolicyValidationError::InvalidPrincipalTarget { entity_id, message }) => {
        // Return 400 Bad Request
        Response {
            status: 400,
            error: format!("Invalid policy: Principal entity {} does not exist", entity_id),
        }
    }
    Err(e) => {
        // Return 400 Bad Request with details
        Response {
            status: 400,
            error: format!("Invalid policy: {}", e),
        }
    }
}
```

**Strategy**: **Reject invalid policies immediately**. Don't allow bad data into the system.

---

## Stage 2: Index Time Validation (Snapshot Generation)

**When**: Indexer building snapshot from graph

**Goal**: Create valid snapshot, handle edge cases gracefully

### Why Re-Validation?

Even though write-time validation exists, issues can still occur:
- Entity deleted after policy was created
- Action deleted after policy was created
- Concurrent modifications
- Data corruption

### Validation Checks

```rust
fn build_snapshot(graph: &Graph) -> Result<Snapshot> {
    let mut valid_policies = Vec::new();
    let mut skipped_policies = Vec::new();

    for policy in graph.get_all_policies() {
        match validate_and_compile_policy(graph, policy) {
            Ok(indexed_policy) => {
                valid_policies.push(indexed_policy);
            }
            Err(e) => {
                log::warn!(
                    "Skipping invalid policy {} ({}): {}",
                    policy.id,
                    policy.name,
                    e
                );

                skipped_policies.push((policy.id, e));

                // Alert operators
                alert_invalid_policy(&policy, &e);

                // Metrics
                metrics::counter!(
                    "arbor.indexer.invalid_policies_skipped",
                    1,
                    "policy_id" => policy.id.to_string(),
                    "policy_type" => format!("{:?}", policy.policy_type),
                    "error_type" => error_type(&e)
                );
            }
        }
    }

    // Log summary
    if !skipped_policies.is_empty() {
        log::error!(
            "Skipped {} invalid policies during snapshot generation",
            skipped_policies.len()
        );
    }

    Ok(Snapshot {
        policies: valid_policies,
        skipped_policies,  // Include for debugging
        // ...
    })
}
```

### Re-Validation Logic

```rust
fn validate_and_compile_policy(graph: &Graph, policy: &Policy) -> Result<IndexedPolicy> {
    // 1. Re-check entity references (may have been deleted)
    validate_entity_references(graph, policy)?;

    // 2. Re-check action references
    validate_action_references(graph, policy)?;

    // 3. Compile condition to bytecode
    let compiled_condition = if let Some(condition) = &policy.conditions {
        Some(compile_condition(condition)?)
    } else {
        None
    };

    // 4. Expand action sets
    let expanded_actions = expand_action_sets(graph, policy)?;

    Ok(IndexedPolicy {
        policy: policy.clone(),
        index: /* assigned later */,
        compiled_condition,
        principal_target: policy.principal.clone(),
        resource_target: policy.resource.clone(),
        expanded_actions,
    })
}
```

### Common Index-Time Issues

**Issue 1: Deleted Entity Reference**
```rust
Policy {
    principal: PolicyTarget::Entity(team_123_uuid),
    ...
}

// But team_123 was deleted from graph
Error: InvalidPrincipalTarget { entity_id: team_123_uuid }

Action: Skip policy, alert operator
```

**Issue 2: Deleted Action Reference**
```rust
Policy {
    actions: vec![edit_action_uuid],
    ...
}

// But edit action was deleted
Error: InvalidAction { action_id: edit_action_uuid }

Action: Skip policy, alert operator
```

**Issue 3: Bytecode Compilation Failure**
```rust
Condition::InNetwork(ip_var, cidr_literal)

// But InNetwork not yet implemented
Error: UnsupportedOperator("InNetwork")

Action: Skip policy, alert operator
```

### Alerting & Monitoring

```rust
fn alert_invalid_policy(policy: &Policy, error: &ValidationError) {
    // Send alert to monitoring system
    alerting::send(Alert {
        severity: Severity::Warning,
        title: "Invalid policy skipped during snapshot generation",
        description: format!(
            "Policy {} ({}) is invalid: {}\n\
             This policy will not be enforced until fixed.",
            policy.id,
            policy.name,
            error
        ),
        labels: {
            "policy_id": policy.id.to_string(),
            "policy_name": policy.name.clone(),
            "policy_type": format!("{:?}", policy.policy_type),
        },
    });
}
```

**Strategy**: **Skip invalid policies with alerts**. Prioritize availability over strict consistency.

---

## Stage 3: Evaluation Time Validation (Authorization)

**When**: Authorizer evaluating policies during check()

**Goal**: Fail safely if policy is somehow invalid at runtime

### Why Re-Validation Again?

This should be **extremely rare** if checksums are working:
- Corrupted bytecode (memory corruption)
- Broken policy references (should be caught by checksum)
- Future: Schema evolution issues

### Evaluation Error Handling

```rust
fn evaluate_conditional_forbids(
    policies: &[IndexedPolicy],
    context: &EvaluationContext,
) -> CheckResponse {
    for policy in policies {
        match evaluate_policy(policy, context) {
            Ok(ConditionResult::True) | Ok(ConditionResult::Unknown) | Ok(ConditionResult::Invalid) => {
                // Apply forbid (including uncertain cases)
                return CheckResponse {
                    decision: Decision::Deny,
                    reason: Some(Reason::ForbiddenBy {
                        policy_id: policy.policy.id,
                        policy_name: policy.policy.name.clone(),
                    }),
                    snapshot_version: context.snapshot_version,
                };
            }
            Ok(ConditionResult::False) => {
                // Don't apply this forbid
                continue;
            }
            Err(e) => {
                // INVALID POLICY at evaluation time
                log::error!(
                    "Policy evaluation error (forbid policy {} '{}'): {}",
                    policy.policy.id,
                    policy.policy.name,
                    e
                );

                metrics::counter!(
                    "arbor.authorizer.policy_evaluation_error",
                    1,
                    "policy_id" => policy.policy.id.to_string(),
                    "policy_type" => "forbid",
                    "error_type" => format!("{:?}", e)
                );

                // FAIL CLOSED: Treat as forbid
                return CheckResponse {
                    decision: Decision::Deny,
                    reason: Some(Reason::PolicyEvaluationError {
                        policy_id: policy.policy.id,
                        message: format!("Policy evaluation failed: {}", e),
                    }),
                    snapshot_version: context.snapshot_version,
                };
            }
        }
    }

    // No forbids applied
    CheckResponse::continue_evaluation()
}

fn evaluate_conditional_permits(
    policies: &[IndexedPolicy],
    context: &EvaluationContext,
) -> Option<CheckResponse> {
    for policy in policies {
        match evaluate_policy(policy, context) {
            Ok(ConditionResult::True) => {
                // Apply permit
                return Some(CheckResponse {
                    decision: Decision::Permit,
                    reason: Some(Reason::PermittedBy {
                        policy_id: policy.policy.id,
                        policy_name: policy.policy.name.clone(),
                    }),
                    snapshot_version: context.snapshot_version,
                });
            }
            Ok(_) => {
                // Don't apply this permit
                continue;
            }
            Err(e) => {
                // INVALID POLICY at evaluation time
                log::error!(
                    "Policy evaluation error (permit policy {} '{}'): {}",
                    policy.policy.id,
                    policy.policy.name,
                    e
                );

                metrics::counter!(
                    "arbor.authorizer.policy_evaluation_error",
                    1,
                    "policy_id" => policy.policy.id.to_string(),
                    "policy_type" => "permit",
                    "error_type" => format!("{:?}", e)
                );

                // FAIL CLOSED: Don't grant access
                continue;
            }
        }
    }

    // No permits applied
    None
}
```

### Evaluation Errors

```rust
pub enum PolicyEvaluationError {
    /// Bytecode is corrupted or invalid
    InvalidBytecode { message: String },

    /// Stack underflow during VM execution
    StackUnderflow,

    /// Unknown opcode
    UnknownOpCode(u8),

    /// Type mismatch during operation
    TypeMismatch { expected: String, found: String },

    /// Division by zero or other arithmetic error
    ArithmeticError(String),
}
```

**Strategy**: **Fail closed**. Invalid forbid → deny, invalid permit → don't grant.

---

## Validation Summary Table

| Stage | When | Invalid Forbid | Invalid Permit | Goal |
|-------|------|----------------|----------------|------|
| **Write Time** | Policy creation | **Reject** ❌ | **Reject** ❌ | Prevent bad data |
| **Index Time** | Snapshot generation | **Skip + Alert** ⚠️ | **Skip + Alert** ⚠️ | Availability |
| **Eval Time** | Authorization check | **Apply forbid** 🔒 | **Don't permit** 🔒 | Security (fail closed) |

---

## Monitoring & Alerting

### Metrics

```rust
// Write time
metrics::counter!("arbor.graph.policy_validation_failures", 1,
    "error_type" => "invalid_entity_reference"
);

// Index time
metrics::counter!("arbor.indexer.invalid_policies_skipped", 1,
    "policy_id" => policy.id.to_string(),
    "policy_type" => "forbid",
    "error_type" => "deleted_entity"
);

// Eval time
metrics::counter!("arbor.authorizer.policy_evaluation_error", 1,
    "policy_id" => policy.id.to_string(),
    "policy_type" => "permit",
    "error_type" => "corrupted_bytecode"
);
```

### Alerts

```yaml
# Index time - policies being skipped
- alert: InvalidPoliciesSkipped
  expr: rate(arbor_indexer_invalid_policies_skipped[5m]) > 0
  for: 1m
  labels:
    severity: warning
  annotations:
    summary: "Indexer skipping invalid policies"
    description: "{{ $value }} invalid policies skipped in last 5 minutes. Check logs for policy IDs."

# Eval time - policies failing at runtime (should be very rare)
- alert: PolicyEvaluationErrors
  expr: rate(arbor_authorizer_policy_evaluation_error[5m]) > 0.01
  for: 5m
  labels:
    severity: critical
  annotations:
    summary: "Policy evaluation errors detected"
    description: "Policies failing to evaluate at runtime. Possible snapshot corruption."
```

---

## Best Practices

### 1. Validate Early

Catch issues at write time when possible. Cheaper to reject than to skip later.

### 2. Use Checksums

Snapshot checksums catch corruption between indexer and authorizer.

### 3. Monitor Skipped Policies

Track which policies are skipped and why. Patterns indicate systemic issues.

### 4. Test Validation

```rust
#[test]
fn test_reject_invalid_policy() {
    let mut graph = Graph::new();

    let policy = Policy {
        principal: PolicyTarget::Entity(non_existent_uuid),
        // ...
    };

    let result = graph.upsert_policy(policy);
    assert!(matches!(result, Err(PolicyValidationError::InvalidPrincipalTarget { .. })));
}
```

### 5. Graceful Degradation

A few skipped policies shouldn't break the entire system. Alert and continue.

### 6. Audit Logs

Log all validation failures for security auditing:

```rust
audit_log::write(AuditEvent::PolicyValidationFailure {
    policy_id: policy.id,
    error: format!("{}", e),
    timestamp: Utc::now(),
    user: request.user,
});
```

---

## Related Documentation

- [Authorization Flow](./authorization-flow.md) - How policies are evaluated
- [Bytecode VM](./bytecode-vm.md) - Condition compilation and execution
- [Data Model](./data-model.md) - Policy structure
- [Architecture](./architecture.md) - System components
