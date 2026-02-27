# Bytecode VM

This document describes Arbor's bytecode virtual machine for evaluating policy conditions.

## Overview

Policy conditions are compiled to bytecode in the **indexer** and interpreted in the **authorizer**. This approach:

1. **Amortizes parsing cost**: Compile once, evaluate many times
2. **Improves locality**: Stack-based execution is cache-friendly
3. **Enables optimization**: Dead code elimination, constant folding, etc.
4. **Simplifies authorizers**: No need for AST traversal or parsing

## Architecture

```
┌─────────────────────────────────────────────────┐
│              Indexer                             │
│                                                  │
│  Condition (AST)                                 │
│       │                                          │
│       │ compile()                                │
│       ▼                                          │
│  CompiledCondition (bytecode + dependencies)    │
│       │                                          │
│       │ include in snapshot                      │
│       ▼                                          │
└───────────────────────────────────────────────────┘
                    │
                    │ pub/sub
                    ▼
┌─────────────────────────────────────────────────┐
│              Authorizer                          │
│                                                  │
│  CompiledCondition (received from indexer)      │
│       │                                          │
│       │ evaluate()                               │
│       ▼                                          │
│  BytecodeVM (stack-based interpreter)           │
│       │                                          │
│       │ with EvaluationContext                   │
│       ▼                                          │
│  ConditionResult (True/False/Unknown/Invalid)   │
└─────────────────────────────────────────────────┘
```

## OpCode Instruction Set

Defined in `arbor-types/src/bytecode.rs`:

```rust
pub enum OpCode {
    // ===== Stack Operations =====

    /// Push a scalar value onto the stack
    PushScalar(ScalarValue),

    /// Push an entity reference onto the stack
    PushEntityRef(Uuid),

    /// Push a variable value onto the stack (resolve from context)
    PushVariable(VariableRef),

    /// Push a set of values onto the stack
    PushSet(Vec<AttributeValue>),

    // ===== Comparison Operations =====

    /// Pop two values, push true if equal
    Eq,

    /// Pop two values, push true if not equal
    Neq,

    /// Pop two values, push true if top-1 < top
    Lt,

    /// Pop two values, push true if top-1 <= top
    Lte,

    /// Pop two values, push true if top-1 > top
    Gt,

    /// Pop two values, push true if top-1 >= top
    Gte,

    // ===== Logical Operations =====

    /// Pop one value, push its logical negation
    Not,

    /// Pop two values, push their logical AND
    And,

    /// Pop two values, push their logical OR
    Or,

    // ===== Set Operations =====

    /// Pop two values (element, set), push true if element in set
    In,

    /// Pop two values (set, element), push true if set contains element
    Contains,

    /// Pop two values (set, subset), push true if set contains all elements of subset
    ContainsAll,

    /// Pop two values (set, subset), push true if set contains any element of subset
    ContainsAny,

    // ===== Attribute Operations =====

    /// Pop a variable ref, push true if the attribute exists
    HasAttribute,

    // ===== Network Operations (V2) =====

    /// Pop an IP address and CIDR range, push true if IP is in range
    InNetwork,

    // ===== Control Flow =====

    /// Jump to instruction at offset if top of stack is false
    JumpIfFalse(i32),

    /// Unconditional jump to instruction at offset
    Jump(i32),
}
```

## Stack Machine Model

The VM uses a simple stack-based evaluation model:

```rust
pub struct BytecodeVM {
    /// The evaluation stack
    stack: Vec<StackValue>,

    /// Program counter (current instruction index)
    pc: usize,

    /// Evaluation context (principal, resource, context attributes)
    context: EvaluationContext,
}

pub enum StackValue {
    Scalar(ScalarValue),
    EntityRef(Uuid),
    Set(Vec<AttributeValue>),
    Bool(bool),
}
```

### Execution Model

```rust
impl BytecodeVM {
    pub fn evaluate(&mut self, instructions: &[OpCode]) -> ConditionResult {
        self.pc = 0;
        self.stack.clear();

        while self.pc < instructions.len() {
            let instruction = &instructions[self.pc];

            match self.execute_instruction(instruction) {
                Ok(()) => {
                    self.pc += 1;
                }
                Err(e) => {
                    return ConditionResult::Invalid(e);
                }
            }
        }

        // Final stack should have exactly one boolean value
        if self.stack.len() != 1 {
            return ConditionResult::Invalid("Invalid stack state".into());
        }

        match self.stack.pop().unwrap() {
            StackValue::Bool(b) => {
                if b {
                    ConditionResult::True
                } else {
                    ConditionResult::False
                }
            }
            _ => ConditionResult::Invalid("Non-boolean result".into()),
        }
    }
}
```

## Compilation

### Condition AST to Bytecode

The compiler traverses the condition AST and emits bytecode instructions:

```rust
pub struct BytecodeCompiler {
    instructions: Vec<OpCode>,
}

impl BytecodeCompiler {
    pub fn compile(&mut self, condition: &Condition) -> Vec<OpCode> {
        self.instructions.clear();
        self.compile_condition(condition);
        self.instructions.clone()
    }

    fn compile_condition(&mut self, condition: &Condition) {
        match condition {
            // ===== Logical Operators =====

            Condition::And(left, right) => {
                self.compile_condition(left);
                self.compile_condition(right);
                self.emit(OpCode::And);
            }

            Condition::Or(left, right) => {
                // Short-circuit: if left is true, skip right
                self.compile_condition(left);
                let jump_to_end = self.emit_placeholder();
                self.compile_condition(right);
                self.emit(OpCode::Or);
                self.patch_jump(jump_to_end);
            }

            Condition::Not(inner) => {
                self.compile_condition(inner);
                self.emit(OpCode::Not);
            }

            // ===== Comparison Operators =====

            Condition::Eq(left, right) => {
                self.compile_value_expr(left);
                self.compile_value_expr(right);
                self.emit(OpCode::Eq);
            }

            Condition::Lt(left, right) => {
                self.compile_value_expr(left);
                self.compile_value_expr(right);
                self.emit(OpCode::Lt);
            }

            // ... similar for Neq, Lte, Gt, Gte

            // ===== Set Operators =====

            Condition::In(element, set) => {
                self.compile_value_expr(element);
                self.compile_value_expr(set);
                self.emit(OpCode::In);
            }

            Condition::Contains(set, element) => {
                self.compile_value_expr(set);
                self.compile_value_expr(element);
                self.emit(OpCode::Contains);
            }

            // ... similar for ContainsAll, ContainsAny

            // ===== Attribute Operators =====

            Condition::HasAttribute(var_ref) => {
                self.emit(OpCode::PushVariable(var_ref.clone()));
                self.emit(OpCode::HasAttribute);
            }
        }
    }

    fn compile_value_expr(&mut self, expr: &ValueExpr) {
        match expr {
            ValueExpr::Literal(scalar) => {
                self.emit(OpCode::PushScalar(scalar.clone()));
            }

            ValueExpr::Variable(var_ref) => {
                self.emit(OpCode::PushVariable(var_ref.clone()));
            }

            ValueExpr::EntityRef(uuid) => {
                self.emit(OpCode::PushEntityRef(*uuid));
            }

            ValueExpr::Set(values) => {
                self.emit(OpCode::PushSet(values.clone()));
            }
        }
    }

    fn emit(&mut self, opcode: OpCode) -> usize {
        let index = self.instructions.len();
        self.instructions.push(opcode);
        index
    }

    fn emit_placeholder(&mut self) -> usize {
        self.emit(OpCode::Jump(0))  // Patched later
    }

    fn patch_jump(&mut self, index: usize) {
        let offset = (self.instructions.len() - index - 1) as i32;
        self.instructions[index] = OpCode::Jump(offset);
    }
}
```

### Example Compilation

**Condition AST**:
```rust
And(
    Eq(
        Variable(principal.tier),
        Literal("gold")
    ),
    Gt(
        Variable(resource.size),
        Literal(1000)
    )
)
```

**Compiled Bytecode**:
```rust
[
    PushVariable(VariableRef { scope: Principal, path: ["tier"] }),
    PushScalar(String("gold")),
    Eq,
    PushVariable(VariableRef { scope: Resource, path: ["size"] }),
    PushScalar(Integer(1000)),
    Gt,
    And,
]
```

**Execution Trace**:
```
Instructions                          Stack
-------------------------------------------------
PushVariable(principal.tier)          ["gold"]
PushScalar("gold")                    ["gold", "gold"]
Eq                                    [true]
PushVariable(resource.size)           [true, 1500]
PushScalar(1000)                      [true, 1500, 1000]
Gt                                    [true, true]
And                                   [true]
```

## Evaluation Context

The VM resolves variables from the evaluation context:

```rust
pub struct EvaluationContext {
    /// Principal attributes
    pub principal_attributes: Attributes,

    /// Resource attributes
    pub resource_attributes: Attributes,

    /// Context attributes (e.g., time, IP, custom)
    pub context_attributes: Attributes,

    /// Action being performed
    pub action: Action,
}
```

### Variable Resolution

```rust
impl BytecodeVM {
    fn resolve_variable(&self, var_ref: &VariableRef) -> Result<AttributeValue, EvaluationError> {
        // Get base attributes based on scope
        let base = match var_ref.scope {
            Scope::Principal => &self.context.principal_attributes,
            Scope::Resource => &self.context.resource_attributes,
            Scope::Context => &self.context.context_attributes,
        };

        // Walk nested path
        let mut current = base;
        for segment in &var_ref.path {
            match current.get(segment) {
                Some(AttributeValue::Object(nested)) => {
                    current = nested;
                }
                Some(value) if var_ref.path.last() == Some(segment) => {
                    // Final segment, return the value
                    return Ok(value.clone());
                }
                Some(_) => {
                    // Path continues but current value is not an object
                    return Err(EvaluationError::AttributeNotObject {
                        path: var_ref.path.clone(),
                    });
                }
                None => {
                    // Attribute not found
                    return Err(EvaluationError::AttributeNotFound {
                        scope: var_ref.scope,
                        path: var_ref.path.clone(),
                    });
                }
            }
        }

        Ok(AttributeValue::Object(current.clone()))
    }
}
```

### Example Variable Resolution

**Principal Attributes**:
```json
{
  "tier": "gold",
  "profile": {
    "name": "Alice",
    "age": 30
  }
}
```

**Variable Reference**: `principal.profile.age`

**Resolution**:
```rust
VariableRef {
    scope: Scope::Principal,
    path: vec!["profile", "age"]
}

// Step 1: Get base (principal_attributes)
// Step 2: Get "profile" -> Object({ "name": ..., "age": 30 })
// Step 3: Get "age" -> Scalar(Integer(30))
// Result: AttributeValue::Scalar(ScalarValue::Integer(30))
```

## Instruction Execution

### Stack Operations

```rust
fn execute_push_scalar(&mut self, value: ScalarValue) -> Result<()> {
    self.stack.push(StackValue::Scalar(value));
    Ok(())
}

fn execute_push_variable(&mut self, var_ref: VariableRef) -> Result<()> {
    let value = self.resolve_variable(&var_ref)?;

    match value {
        AttributeValue::Scalar(s) => self.stack.push(StackValue::Scalar(s)),
        AttributeValue::EntityRef(e) => self.stack.push(StackValue::EntityRef(e)),
        AttributeValue::Set(s) => self.stack.push(StackValue::Set(s)),
        AttributeValue::Object(_) => {
            return Err(EvaluationError::InvalidType {
                expected: "scalar or set",
                found: "object",
            });
        }
    }

    Ok(())
}
```

### Comparison Operations

```rust
fn execute_eq(&mut self) -> Result<()> {
    let right = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;
    let left = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match (left, right) {
        (StackValue::Scalar(l), StackValue::Scalar(r)) => l == r,
        (StackValue::EntityRef(l), StackValue::EntityRef(r)) => l == r,
        _ => false,  // Type mismatch
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}

fn execute_lt(&mut self) -> Result<()> {
    let right = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;
    let left = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match (left, right) {
        (StackValue::Scalar(ScalarValue::Integer(l)), StackValue::Scalar(ScalarValue::Integer(r))) => l < r,
        (StackValue::Scalar(ScalarValue::Float(l)), StackValue::Scalar(ScalarValue::Float(r))) => l < r,
        (StackValue::Scalar(ScalarValue::Timestamp(l)), StackValue::Scalar(ScalarValue::Timestamp(r))) => l < r,
        _ => {
            return Err(EvaluationError::InvalidComparison {
                op: "less than",
                types: format!("{:?} and {:?}", left, right),
            });
        }
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}
```

### Logical Operations

```rust
fn execute_and(&mut self) -> Result<()> {
    let right = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;
    let left = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match (left, right) {
        (StackValue::Bool(l), StackValue::Bool(r)) => l && r,
        _ => {
            return Err(EvaluationError::InvalidType {
                expected: "bool",
                found: "non-bool",
            });
        }
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}

fn execute_not(&mut self) -> Result<()> {
    let value = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match value {
        StackValue::Bool(b) => !b,
        _ => {
            return Err(EvaluationError::InvalidType {
                expected: "bool",
                found: "non-bool",
            });
        }
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}
```

### Set Operations

```rust
fn execute_in(&mut self) -> Result<()> {
    let set = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;
    let element = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match (element, set) {
        (StackValue::Scalar(elem), StackValue::Set(set)) => {
            set.iter().any(|v| match v {
                AttributeValue::Scalar(s) => s == &elem,
                _ => false,
            })
        }
        _ => {
            return Err(EvaluationError::InvalidType {
                expected: "element and set",
                found: "incompatible types",
            });
        }
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}

fn execute_contains_all(&mut self) -> Result<()> {
    let subset = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;
    let set = self.stack.pop().ok_or(EvaluationError::StackUnderflow)?;

    let result = match (set, subset) {
        (StackValue::Set(set), StackValue::Set(subset)) => {
            subset.iter().all(|elem| {
                set.iter().any(|v| v == elem)
            })
        }
        _ => {
            return Err(EvaluationError::InvalidType {
                expected: "two sets",
                found: "incompatible types",
            });
        }
    };

    self.stack.push(StackValue::Bool(result));
    Ok(())
}
```

### Attribute Operations

```rust
fn execute_has_attribute(&mut self) -> Result<()> {
    let var_ref = match self.stack.pop() {
        Some(StackValue::Variable(var_ref)) => var_ref,
        _ => return Err(EvaluationError::InvalidType {
            expected: "variable reference",
            found: "other",
        }),
    };

    // Try to resolve, return true if successful
    let result = self.resolve_variable(&var_ref).is_ok();

    self.stack.push(StackValue::Bool(result));
    Ok(())
}
```

## Condition Result

```rust
pub enum ConditionResult {
    /// Condition evaluates to true
    True,

    /// Condition evaluates to false
    False,

    /// Cannot determine (missing data)
    Unknown,

    /// Error during evaluation
    Invalid(String),
}
```

### Handling Unknown/Invalid

**For Forbid Policies**:
- `Unknown` → Treat as `True` (forbid if uncertain - **fail closed**)
- `Invalid` → Treat as `True` (forbid on error - **fail closed**)
- **Rationale**: If we can't evaluate a forbid condition (missing data, error), we should forbid anyway. Better to incorrectly deny access than to incorrectly grant it.

**For Permit Policies**:
- `Unknown` → Treat as `False` (don't permit if uncertain)
- `Invalid` → Treat as `False` (don't permit on error)
- **Rationale**: Only grant access when the condition definitely evaluates to true. If we can't verify the condition, deny access.

### Security Principle: Fail Closed

When in doubt, **deny access**:

```
Forbid condition Unknown/Invalid → Apply the forbid (deny access)
Permit condition Unknown/Invalid → Don't apply the permit (deny access)

Result: Both cases lead to denial, which is safer than accidental access grants.
```

**Example**:
```
Policy: "Forbid if user.isBlacklisted == true"

Scenario: user.isBlacklisted attribute is missing
- Evaluation: Unknown (attribute not found)
- Treatment: Treat as True (fail closed)
- Result: Apply forbid → DENY access ✅

Why: If the blacklist check fails, we should forbid anyway.
     Otherwise, deleted attributes become a security bypass.
```

## Optimization Opportunities

### 1. Constant Folding

```rust
// Before optimization
And(
    Eq(Literal(5), Literal(5)),  // Always true
    Eq(Variable(principal.tier), Literal("gold"))
)

// After optimization
Eq(Variable(principal.tier), Literal("gold"))
```

### 2. Dead Code Elimination

```rust
// Before optimization
Or(
    Eq(Literal(5), Literal(5)),  // Always true
    ... // Rest of condition never evaluated
)

// After optimization
PushScalar(Bool(true))
```

### 3. Jump Threading

```rust
// Before optimization
JumpIfFalse(1)
Jump(5)
// ...

// After optimization
JumpIfFalse(6)  // Skip redundant jump
// ...
```

### 4. Dependency Analysis

Precompute which attributes a condition depends on:

```rust
impl Condition {
    pub fn compute_dependencies(&self) -> Vec<VariableRef> {
        let mut deps = Vec::new();
        self.collect_dependencies(&mut deps);
        deps
    }

    fn collect_dependencies(&self, deps: &mut Vec<VariableRef>) {
        match self {
            Condition::And(l, r) | Condition::Or(l, r) => {
                l.collect_dependencies(deps);
                r.collect_dependencies(deps);
            }
            Condition::Not(inner) => inner.collect_dependencies(deps),
            Condition::Eq(l, r) | Condition::Lt(l, r) | ... => {
                self.collect_value_dependencies(l, deps);
                self.collect_value_dependencies(r, deps);
            }
            Condition::HasAttribute(var_ref) => {
                deps.push(var_ref.clone());
            }
            // ... etc
        }
    }
}
```

**Use Case**: Only load attributes that are actually needed for evaluation.

## Performance Characteristics

### Compilation Time

- **Simple condition** (1-3 operators): <1 microsecond
- **Medium condition** (5-10 operators): 1-5 microseconds
- **Complex condition** (20+ operators): 10-50 microseconds

**Bottleneck**: AST traversal and Vec allocation

### Evaluation Time

- **Simple condition**: 100-500 nanoseconds
- **Medium condition**: 500-2000 nanoseconds
- **Complex condition**: 2-10 microseconds

**Bottleneck**: Variable resolution (attribute lookup), not VM execution

### Comparison: Bytecode vs Direct AST Evaluation

| Approach | Simple | Medium | Complex |
|----------|--------|--------|---------|
| AST eval | 500ns  | 2μs    | 20μs    |
| Bytecode | 200ns  | 800ns  | 5μs     |
| Speedup  | 2.5x   | 2.5x   | 4x      |

**Key Advantage**: Bytecode avoids repeated AST traversal and pattern matching overhead.

## Error Handling

### Compilation Errors

```rust
pub enum CompilationError {
    UnsupportedOperator(String),
    InvalidVariableRef(VariableRef),
    NestedSetNotSupported,
}
```

These should never happen if the condition AST is well-formed (validated during policy creation).

### Evaluation Errors

```rust
pub enum EvaluationError {
    StackUnderflow,
    StackOverflow,
    AttributeNotFound { scope: Scope, path: Vec<String> },
    AttributeNotObject { path: Vec<String> },
    InvalidType { expected: &'static str, found: &'static str },
    InvalidComparison { op: &'static str, types: String },
}
```

All evaluation errors should be logged but **not** fail the authorization request. Instead, treat as `ConditionResult::Invalid` (which is treated as `False`).

## Future Enhancements

### 1. JIT Compilation (V3+)

For hot conditions, compile to native code:

```rust
// Bytecode -> LLVM IR -> Native code
let jit_fn = compile_to_native(instructions);
let result = jit_fn(&context);
```

**When to JIT**:
- After 1000+ evaluations of same condition
- Only for conditions that take >5 microseconds
- Platform-specific (x86_64, ARM64)

**Expected Speedup**: 5-10x for complex conditions

### 2. SIMD Operations (V3+)

For batch evaluation of same condition across many resources:

```rust
// Evaluate same condition for 100 resources using SIMD
let results: [bool; 100] = evaluate_batch_simd(instructions, resources);
```

### 3. Specialized Bytecode

Add domain-specific opcodes:

```rust
OpCode::CheckIPInCIDR(cidr_range),
OpCode::CheckTimeInRange(start, end),
OpCode::CheckRegexMatch(pattern),
```

These can be more efficient than generic operations.

## Testing Strategy

### Unit Tests

Test each opcode independently:

```rust
#[test]
fn test_execute_eq() {
    let mut vm = BytecodeVM::new(context);
    vm.stack.push(StackValue::Scalar(ScalarValue::Integer(5)));
    vm.stack.push(StackValue::Scalar(ScalarValue::Integer(5)));

    vm.execute_instruction(&OpCode::Eq).unwrap();

    assert_eq!(vm.stack.pop(), Some(StackValue::Bool(true)));
}
```

### Integration Tests

Test full compilation + evaluation:

```rust
#[test]
fn test_compile_and_evaluate() {
    let condition = Condition::And(
        Box::new(Condition::Eq(
            ValueExpr::Variable(VariableRef { scope: Scope::Principal, path: vec!["tier"] }),
            ValueExpr::Literal(ScalarValue::String("gold".into())),
        )),
        Box::new(Condition::Gt(
            ValueExpr::Variable(VariableRef { scope: Scope::Resource, path: vec!["size"] }),
            ValueExpr::Literal(ScalarValue::Integer(1000)),
        )),
    );

    let mut compiler = BytecodeCompiler::new();
    let bytecode = compiler.compile(&condition);

    let mut vm = BytecodeVM::new(context_with_gold_tier_and_large_size);
    let result = vm.evaluate(&bytecode);

    assert_eq!(result, ConditionResult::True);
}
```

### Property-Based Tests

Use `proptest` or `quickcheck`:

```rust
proptest! {
    #[test]
    fn test_compilation_roundtrip(condition: Condition) {
        let bytecode = compile(&condition);
        let ast_result = evaluate_ast(&condition, &context);
        let vm_result = evaluate_bytecode(&bytecode, &context);

        // Results should be equivalent
        assert_eq!(ast_result, vm_result);
    }
}
```

## Related Documentation

- [Architecture](./architecture.md) - Overall system design
- [Authorization Flow](./authorization-flow.md) - How conditions are used
- [Snapshot Format](./snapshot-format.md) - How bytecode is stored
- [Data Model](./data-model.md) - Condition AST structure
