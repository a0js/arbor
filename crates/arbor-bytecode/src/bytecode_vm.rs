//! Bytecode VM for Policy Condition Evaluation
//!
//! Stack-based interpreter that executes OpCode instructions.
//! Variables are resolved from the EvaluationContext at evaluation time.
//!
//! ## Missing attribute semantics
//!
//! When a variable path does not exist on an entity, `PushVariable` pushes
//! `StackValue::Missing` instead of erroring. Every comparison opcode treats
//! `Missing` on either operand as `false` (the missing value never matches).
//!
//! Compiler invariant: `PushVariable` must always be immediately consumed by a
//! comparison or set opcode (`Eq`, `Neq`, `Lt`, `Lte`, `Gt`, `Gte`, `In`,
//! `Contains`, `ContainsAll`, `ContainsAny`). Logical operators (`And`, `Or`,
//! `Not`) must never see `Missing`; if they do the VM returns `Invalid`.
//!
//! ## Neq is not eq+not
//!
//! `Neq` has its own `Missing` arm. Delegating to `execute_eq` + `execute_not`
//! would turn `Missing == X → false` into `NOT false → true`, creating an
//! authorization bypass for conditions like `permit if principal.tier != "restricted"`.

use arbor_types::{
    Attributes, AttributeValue, AttributeValueView, ConditionResult, EntityResolver,
    EntityTypeId, EvaluationContext, EvaluationError, OpCode,
    ResolvedEntityIndex, VariableRef, VariableScope,
};
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use ordered_float::OrderedFloat;
use std::cmp::Ordering;
use std::net::IpAddr;

#[derive(Debug, Clone, PartialEq)]
pub enum StackValue {
    Integer(i64),
    Float(OrderedFloat<f64>),
    Timestamp(DateTime<Utc>),
    String(String),
    Bool(bool),
    EntityRef(u32),
    IpAddr(IpAddr),
    IpNetwork(IpNet),
    Set(Vec<AttributeValue>),
    Missing,
}

/// Bytecode VM for condition evaluation
pub struct BytecodeVM {
    stack: Vec<StackValue>,
}

impl BytecodeVM {
    pub fn new() -> Self {
        Self {
            stack: Vec::with_capacity(16),
        }
    }

    /// Evaluate a compiled condition.
    ///
    /// Returns `True`/`False` on success, or `Invalid` on type errors or
    /// compiler invariant violations. Missing attributes evaluate to `false`
    /// at comparison boundaries; they are not errors.
    pub fn evaluate(&mut self, instructions: &[OpCode], ctx: &EvaluationContext<'_>) -> ConditionResult {
        self.stack.clear();
        let mut pc: usize = 0;

        while pc < instructions.len() {
            match &instructions[pc] {
                OpCode::Jump(target) => {
                    let target = *target as usize;
                    if target > instructions.len() {
                        return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                            format!("pc={}: Jump target {} out of bounds (len={})", pc, target, instructions.len()),
                        )]);
                    }
                    pc = target;
                    continue; // don't increment pc
                }
                OpCode::JumpIfFalse(target) => {
                    let target_usize = *target as usize;
                    if target_usize > instructions.len() {
                        return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                            format!("pc={}: JumpIfFalse target {} out of bounds", pc, target_usize),
                        )]);
                    }
                    match self.pop() {
                        Ok(StackValue::Bool(false)) => {
                            pc = target_usize;
                            continue; // jump taken
                        }
                        Ok(StackValue::Bool(true)) => {
                            // fall through, pc will increment below
                        }
                        Ok(StackValue::Missing) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: compiler bug: Missing reached JumpIfFalse", pc),
                            )]);
                        }
                        Ok(other) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: JumpIfFalse requires Bool, got {:?}", pc, other),
                            )]);
                        }
                        Err(e) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: {}", pc, e),
                            )]);
                        }
                    }
                }
                OpCode::JumpIfTrue(target) => {
                    let target_usize = *target as usize;
                    if target_usize > instructions.len() {
                        return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                            format!("pc={}: JumpIfTrue target {} out of bounds", pc, target_usize),
                        )]);
                    }
                    match self.pop() {
                        Ok(StackValue::Bool(true)) => {
                            pc = target_usize;
                            continue;
                        }
                        Ok(StackValue::Bool(false)) => { /* fall through */ }
                        Ok(StackValue::Missing) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: compiler bug: Missing reached JumpIfTrue", pc),
                            )]);
                        }
                        Ok(other) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: JumpIfTrue requires Bool, got {:?}", pc, other),
                            )]);
                        }
                        Err(e) => {
                            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                                format!("pc={}: {}", pc, e),
                            )]);
                        }
                    }
                }
                other => {
                    if let Err(e) = self.execute_instruction(other, ctx) {
                        return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                            format!("pc={}: {}", pc, e),
                        )]);
                    }
                }
            }
            pc += 1;
        }

        // Final stack check (unchanged)
        if self.stack.len() != 1 {
            return ConditionResult::Invalid(vec![EvaluationError::ExecutionError(format!(
                "invalid final stack: {} values (expected 1)",
                self.stack.len()
            ))]);
        }

        match self.stack.pop().unwrap() {
            StackValue::Bool(true) => ConditionResult::True,
            StackValue::Bool(false) => ConditionResult::False,
            StackValue::Missing => ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                "compiler bug: PushVariable not consumed by a comparison".into(),
            )]),
            _ => ConditionResult::Invalid(vec![EvaluationError::ExecutionError(
                "non-boolean result on stack".into(),
            )]),
        }
    }


    fn execute_instruction(&mut self, instruction: &OpCode, ctx: &EvaluationContext<'_>) -> Result<(), String> {
        match instruction {
            OpCode::PushInteger(i) => {
                self.stack.push(StackValue::Integer(*i));
                Ok(())
            },
            OpCode::PushFloat(f) => {
                self.stack.push(StackValue::Float(*f));
                Ok(())
            },
            OpCode::PushTimestamp(t) => {
                self.stack.push(StackValue::Timestamp(*t));
                Ok(())
            },
            OpCode::PushString(s) => {
                self.stack.push(StackValue::String(s.clone()));
                Ok(())
            },
            OpCode::PushBool(b) => {
                self.stack.push(StackValue::Bool(*b));
                Ok(())
            }
            OpCode::PushIpAddr(ip) => {
                self.stack.push(StackValue::IpAddr(*ip));
                Ok(())
            },
            OpCode::PushIpNetwork(net) => {
                self.stack.push(StackValue::IpNetwork(*net));
                Ok(())
            },
            OpCode::PushEntityRef(idx) => {
                self.stack.push(StackValue::EntityRef(*idx));
                Ok(())
            }
            OpCode::PushVariable(var_ref) => self.execute_push_variable(var_ref, ctx),
            OpCode::PushSet(values) => {
                self.stack.push(StackValue::Set(values.clone()));
                Ok(())
            }
            OpCode::Eq => self.execute_eq(),
            OpCode::Neq => self.execute_neq(),
            OpCode::Lt => self.execute_lt(),
            OpCode::Lte => self.execute_lte(),
            OpCode::Gt => self.execute_gt(),
            OpCode::Gte => self.execute_gte(),
            OpCode::And => self.execute_and(),
            OpCode::Or => self.execute_or(),
            OpCode::Not => self.execute_not(),
            OpCode::In => self.execute_in(),
            OpCode::Contains => self.execute_contains(),
            OpCode::ContainsAll => self.execute_contains_all(),
            OpCode::ContainsAny => self.execute_contains_any(),
            OpCode::HasAttribute(var_ref) => self.execute_has_attribute(var_ref, ctx),
            OpCode::StartsWith => self.execute_starts_with(),
            OpCode::EndsWith => self.execute_ends_with(),
            OpCode::StringContains => self.execute_string_contains(),
            OpCode::Like => self.execute_like(),
            OpCode::IsType(scope, type_id) => self.execute_is_type(scope, type_id, ctx),
            OpCode::InNetwork => self.execute_in_network(),
            OpCode::InHierarchy(descendant, ancestor) => self.execute_in_hierarchy(descendant, ancestor, ctx),
            OpCode::JumpIfFalse(_) | OpCode::Jump(_) | OpCode::JumpIfTrue(_) => {
                Err("control flow shouldn't evaluate here".into())
            }
        }
    }

    // ===== Stack Operations =====

    fn execute_push_variable(&mut self, var_ref: &VariableRef, ctx: &EvaluationContext<'_>) -> Result<(), String> {
        // When the path is empty the variable refers to the entity itself (synthesised EntityRef).
        let value = if var_ref.path.is_empty() {
            match var_ref.scope {
                VariableScope::Principal => StackValue::EntityRef(ctx.principal.idx),
                VariableScope::Resource => StackValue::EntityRef(ctx.resource.idx),
                VariableScope::Context => StackValue::Missing,
            }
        } else {
            self.resolve_variable(var_ref, ctx).unwrap_or(StackValue::Missing)
        };
        self.stack.push(value);
        Ok(())
    }

    /// Resolve an attribute path directly to a `StackValue`. Returns `None`
    /// if the path does not exist or if context attributes are absent for a
    /// Context-scoped variable.
    ///
    /// Principal/Resource attributes are resolved through `ctx.entities`'
    /// shared attribute arena (`IndexedAttributeValue`); Context attributes
    /// are a plain, per-request `Attributes` (never persisted, so never
    /// arena-backed) -- the two scopes need genuinely different resolution
    /// paths, unified here into one `StackValue` result.
    ///
    /// Callers that need to handle the empty-path (entity-self) case must do so
    /// before calling this function; `resolve_variable` returns `None` for an
    /// empty path.
    fn resolve_variable(&self, var_ref: &VariableRef, ctx: &EvaluationContext<'_>) -> Option<StackValue> {
        if var_ref.path.is_empty() {
            return None;
        }
        match var_ref.scope {
            VariableScope::Context => {
                let value = ctx.context_attrs?.get_nested(&var_ref.path)?;
                Some(Self::attribute_value_to_stack(value))
            }
            VariableScope::Principal | VariableScope::Resource => {
                let base = match var_ref.scope {
                    VariableScope::Principal => ctx.principal.attributes,
                    VariableScope::Resource => ctx.resource.attributes,
                    VariableScope::Context => unreachable!(),
                };
                let value = ctx.entities.resolve_attribute_path(base, &var_ref.path)?;
                Some(Self::attribute_value_view_to_stack(ctx.entities, value))
            }
        }
    }

    fn attribute_value_to_stack(v: &AttributeValue) -> StackValue {
        match v {
            AttributeValue::String(s) => StackValue::String(s.clone()),
            AttributeValue::Float(f) => StackValue::Float(*f),
            AttributeValue::Integer(i) => StackValue::Integer(*i),
            AttributeValue::Bool(b) => StackValue::Bool(*b),
            AttributeValue::IpAddr(ip) => StackValue::IpAddr(*ip),
            AttributeValue::IpNetwork(net) => StackValue::IpNetwork(*net),
            AttributeValue::Timestamp(t) => StackValue::Timestamp(*t),
            AttributeValue::EntityRef(u) => StackValue::EntityRef(*u),
            AttributeValue::Set(s) => StackValue::Set(s.clone()),
            // Objects cannot be directly compared; treat as Missing.
            AttributeValue::Object(_) => StackValue::Missing,
        }
    }

    fn attribute_value_view_to_stack(entities: &dyn EntityResolver, v: AttributeValueView<'_>) -> StackValue {
        match v {
            AttributeValueView::String(s) => StackValue::String(s.to_string()),
            AttributeValueView::Float(f) => StackValue::Float(OrderedFloat(f)),
            AttributeValueView::Integer(i) => StackValue::Integer(i),
            AttributeValueView::Bool(b) => StackValue::Bool(b),
            AttributeValueView::IpAddr(ip) => StackValue::IpAddr(ip),
            AttributeValueView::IpNetwork(net) => StackValue::IpNetwork(net),
            AttributeValueView::Timestamp(t) => StackValue::Timestamp(t),
            AttributeValueView::EntityRef(u) => StackValue::EntityRef(u),
            AttributeValueView::Set(set_ref) => StackValue::Set(
                entities
                    .attribute_set_values(set_ref)
                    .into_iter()
                    .map(|e| Self::attribute_value_view_to_owned(entities, e))
                    .collect(),
            ),
            // Objects cannot be directly compared; treat as Missing.
            AttributeValueView::Object(_) => StackValue::Missing,
        }
    }

    /// Converts a borrowed `AttributeValueView` into an owned
    /// `AttributeValue` -- needed only to materialize `Set` elements onto
    /// `StackValue::Set(Vec<AttributeValue>)`, which stays the pre-arena type
    /// since `Set`/`ContainsAll`/`ContainsAny` are unaffected by this change.
    /// Recurses through `entities` for the (rare) case of a nested
    /// `Object`/`Set` inside a `Set`.
    fn attribute_value_view_to_owned(entities: &dyn EntityResolver, v: AttributeValueView<'_>) -> AttributeValue {
        match v {
            AttributeValueView::String(s) => AttributeValue::String(s.to_string()),
            AttributeValueView::Float(f) => AttributeValue::Float(OrderedFloat(f)),
            AttributeValueView::Integer(i) => AttributeValue::Integer(i),
            AttributeValueView::Bool(b) => AttributeValue::Bool(b),
            AttributeValueView::IpAddr(ip) => AttributeValue::IpAddr(ip),
            AttributeValueView::IpNetwork(net) => AttributeValue::IpNetwork(net),
            AttributeValueView::Timestamp(t) => AttributeValue::Timestamp(t),
            AttributeValueView::EntityRef(u) => AttributeValue::EntityRef(u),
            AttributeValueView::Set(set_ref) => AttributeValue::Set(
                entities
                    .attribute_set_values(set_ref)
                    .into_iter()
                    .map(|e| Self::attribute_value_view_to_owned(entities, e))
                    .collect(),
            ),
            AttributeValueView::Object(obj_ref) => {
                let mut attrs = Attributes::new();
                for (name, value) in entities.attribute_pairs_view(obj_ref) {
                    attrs.set(name, Self::attribute_value_view_to_owned(entities, value));
                }
                AttributeValue::Object(attrs)
            }
        }
    }

    // ===== Attribute Operations =====

    fn execute_has_attribute(&mut self, var_ref: &VariableRef, ctx: &EvaluationContext<'_>) -> Result<(), String> {
        let exists = if var_ref.path.is_empty() {
            // Empty path refers to the entity itself; Principal/Resource always exist.
            matches!(var_ref.scope, VariableScope::Principal | VariableScope::Resource)
        } else {
            self.resolve_variable(var_ref, ctx).is_some()
        };
        self.stack.push(StackValue::Bool(exists));
        Ok(())
    }

    // ===== Comparison Operations =====

    fn execute_eq(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                Self::stack_scalar_eq(&left, &right)
            }
            (StackValue::EntityRef(l), StackValue::EntityRef(r)) => l == r,
            (StackValue::Bool(l), StackValue::Bool(r)) => l == r,
            (StackValue::IpAddr(l), StackValue::IpAddr(r)) => l == r,
            _ => return Err(format!("type mismatch in ==: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_neq(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        // Must not delegate to execute_eq + execute_not: Missing == X → false,
        // then NOT false → true, which would be an authorization bypass.
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                !Self::stack_scalar_eq(&left, &right)
            }
            (StackValue::EntityRef(l), StackValue::EntityRef(r)) => l != r,
            (StackValue::Bool(l), StackValue::Bool(r)) => l != r,
            (StackValue::IpAddr(l), StackValue::IpAddr(r)) => l != r,
            _ => return Err(format!("type mismatch in !=: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_lt(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                Self::stack_scalar_cmp(&left, &right)? == Ordering::Less
            }
            _ => return Err(format!("type mismatch in <: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_lte(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                Self::stack_scalar_cmp(&left, &right)? != Ordering::Greater
            }
            _ => return Err(format!("type mismatch in <=: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_gt(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                Self::stack_scalar_cmp(&left, &right)? == Ordering::Greater
            }
            _ => return Err(format!("type mismatch in >: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_gte(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (&left, &right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Integer(_), StackValue::Integer(_))
            | (StackValue::Integer(_), StackValue::Float(_))
            | (StackValue::Float(_), StackValue::Integer(_))
            | (StackValue::Float(_), StackValue::Float(_))
            | (StackValue::String(_), StackValue::String(_))
            | (StackValue::Timestamp(_), StackValue::Timestamp(_)) => {
                Self::stack_scalar_cmp(&left, &right)? != Ordering::Less
            }
            _ => return Err(format!("type mismatch in >=: {:?} vs {:?}", left, right)),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    // ===== Logical Operations =====

    fn execute_and(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (left, right) {
            (StackValue::Bool(l), StackValue::Bool(r)) => l && r,
            (StackValue::Missing, _) | (_, StackValue::Missing) => {
                return Err(
                    "compiler bug: Missing reached And — PushVariable must be consumed by a comparison".into(),
                );
            }
            _ => return Err("invalid types for And (expected bool)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_or(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        let result = match (left, right) {
            (StackValue::Bool(l), StackValue::Bool(r)) => l || r,
            (StackValue::Missing, _) | (_, StackValue::Missing) => {
                return Err(
                    "compiler bug: Missing reached Or — PushVariable must be consumed by a comparison".into(),
                );
            }
            _ => return Err("invalid types for Or (expected bool)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_not(&mut self) -> Result<(), String> {
        let value = self.pop()?;
        let result = match value {
            StackValue::Bool(b) => !b,
            StackValue::Missing => {
                return Err(
                    "compiler bug: Missing reached Not — PushVariable must be consumed by a comparison".into(),
                );
            }
            _ => return Err("invalid type for Not (expected bool)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    // ===== Set Operations =====

    /// Stack: `[..., element, set]` → `[..., bool]`
    /// Evaluates: `element ∈ set`
    fn execute_in(&mut self) -> Result<(), String> {
        let set = self.pop()?;
        let element = self.pop()?;
        let result = match (element, set) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (elem, StackValue::Set(set_vals)) => set_vals
                .iter()
                .any(|v| Self::stack_val_attribute_val_eq(&elem, v)),
            _ => return Err("In requires (element, set)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    /// Stack: `[..., set, element]` → `[..., bool]`
    /// Evaluates: `element ∈ set` (operand order is reversed from `In`)
    fn execute_contains(&mut self) -> Result<(), String> {
        let element = self.pop()?;
        let set = self.pop()?;
        let result = match (set, element) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Set(set_vals), elem) => set_vals
                .iter()
                .any(|v| Self::stack_val_attribute_val_eq(&elem, v)),
            _ => return Err("Contains requires (set, element)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_contains_all(&mut self) -> Result<(), String> {
        let subset = self.pop()?;
        let set = self.pop()?;
        let result = match (set, subset) {
            (StackValue::Set(set_vals), StackValue::Set(subset_vals)) => subset_vals
                .iter()
                .all(|sub| set_vals.iter().any(|s| Self::attribute_value_eq(s, sub))),
            (StackValue::Set(_), StackValue::Missing)
            | (StackValue::Missing, StackValue::Set(_))
            | (StackValue::Missing, StackValue::Missing) => false,
            _ => return Err("ContainsAll requires two sets".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_contains_any(&mut self) -> Result<(), String> {
        let subset = self.pop()?;
        let set = self.pop()?;
        let result = match (set, subset) {
            (StackValue::Set(set_vals), StackValue::Set(subset_vals)) => subset_vals
                .iter()
                .any(|sub| set_vals.iter().any(|s| Self::attribute_value_eq(s, sub))),
            (StackValue::Set(_), StackValue::Missing)
            | (StackValue::Missing, StackValue::Set(_))
            | (StackValue::Missing, StackValue::Missing) => false,
            _ => return Err("ContainsAny requires two sets".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    // ===== String Operations =====

    /// Stack: `[..., string, prefix]` → `[..., bool]`
    fn execute_starts_with(&mut self) -> Result<(), String> {
        let prefix = self.pop()?;
        let string = self.pop()?;
        let result = match (string, prefix) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::String(s), StackValue::String(p)) => s.starts_with(p.as_str()),
            _ => return Err("StartsWith requires (string, string)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    /// Stack: `[..., string, suffix]` → `[..., bool]`
    fn execute_ends_with(&mut self) -> Result<(), String> {
        let suffix = self.pop()?;
        let string = self.pop()?;
        let result = match (string, suffix) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::String(s), StackValue::String(p)) => s.ends_with(p.as_str()),
            _ => return Err("EndsWith requires (string, string)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    /// Stack: `[..., haystack, needle]` → `[..., bool]`
    fn execute_string_contains(&mut self) -> Result<(), String> {
        let needle = self.pop()?;
        let haystack = self.pop()?;
        let result = match (haystack, needle) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::String(s), StackValue::String(n)) => s.contains(n.as_str()),
            _ => return Err("StringContains requires (string, string)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    /// Stack: `[..., string, pattern]` → `[..., bool]`
    ///
    /// Glob matching: `*` matches any sequence of characters (including empty).
    /// No `?`, no character classes, no escaping. Missing on either → false.
    fn execute_like(&mut self) -> Result<(), String> {
        let pattern = self.pop()?;
        let string = self.pop()?;
        let result = match (string, pattern) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::String(s), StackValue::String(p)) => Self::glob_match(&s, &p),
            _ => return Err("Like requires (string, pattern)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    // ===== Entity Type Check =====

    /// Resolve a `ResolvedEntityIndex` to a concrete `u32` snapshot index.
    ///
    /// For `Variable` operands with an empty path the entity index is taken
    /// directly from the evaluation context (principal / resource). For
    /// non-empty paths the attribute value must be an `EntityRef`.
    fn resolve_entity_index(
        &self,
        operand: &ResolvedEntityIndex,
        ctx: &EvaluationContext<'_>,
    ) -> Result<u32, String> {
        match operand {
            ResolvedEntityIndex::Direct(idx) => Ok(*idx),
            ResolvedEntityIndex::Variable(var_ref) => {
                if var_ref.path.is_empty() {
                    match var_ref.scope {
                        VariableScope::Principal => Ok(ctx.principal.idx),
                        VariableScope::Resource => Ok(ctx.resource.idx),
                        VariableScope::Context => Err("Context scope cannot be used as entity ref in InHierarchy".into()),
                    }
                } else {
                    let Some(StackValue::EntityRef(ent)) = self.resolve_variable(var_ref, ctx) else {
                        return Err("Variable must resolve to EntityRef".into());
                    };
                    Ok(ent)
                }
            }
        }
    }

    fn execute_in_hierarchy(&mut self, descendant: &ResolvedEntityIndex, ancestor: &ResolvedEntityIndex, ctx: &EvaluationContext<'_>) -> Result<(), String> {
        let desc_idx = self.resolve_entity_index(descendant, ctx)?;
        let anc_idx = self.resolve_entity_index(ancestor, ctx)?;

        let Some(ancestors) = ctx.entities.ancestors_of(desc_idx) else {
            return Err("Entity not found".into());
        };

        self.stack.push(StackValue::Bool(ancestors.binary_search(&anc_idx).is_ok()));
        Ok(())
    }

    /// Checks whether an IP address is contained in a network.
    ///
    /// Stack: `[..., IpAddr, IpNetwork]` → `[..., Bool]`
    ///
    /// Missing on either operand → `false` (attribute absent, no match).
    /// Wrong types → `Invalid` (compiler bug or bad policy).
    fn execute_in_network(&mut self) -> Result<(), String> {
        let right = self.pop()?;
        let left = self.pop()?;
        match (left, right) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => {
                self.stack.push(StackValue::Bool(false));
            }
            (StackValue::IpAddr(ip), StackValue::IpNetwork(net)) => {
                self.stack.push(StackValue::Bool(net.contains(&ip)));
            }
            (l, r) => {
                return Err(format!(
                    "InNetwork: expected (IpAddr, IpNetwork), got ({:?}, {:?})",
                    l, r
                ));
            }
        }
        Ok(())
    }

    /// Pushes `Bool(true)` if the entity at `scope` has `entity_type == type_id`.
    /// Never produces Missing. The entity is always present in the evaluation context.
    fn execute_is_type(&mut self, scope: &VariableScope, type_id: &EntityTypeId, ctx: &EvaluationContext<'_>) -> Result<(), String> {
        let entity_type = match scope {
            VariableScope::Principal => ctx.principal.entity_type,
            VariableScope::Resource => ctx.resource.entity_type,
            VariableScope::Context => return Err("IsType is not valid on Context scope".into()),
        };
        self.stack.push(StackValue::Bool(entity_type == *type_id));
        Ok(())
    }

    // ===== Pure Helper Functions =====

    fn pop(&mut self) -> Result<StackValue, String> {
        self.stack.pop().ok_or_else(|| "stack underflow".into())
    }

    /// Scalar equality with int/float coercion.
    ///
    /// Note: `i64 as f64` is lossy above 2^53. Integers outside ±2^53 compared
    /// against float literals may produce incorrect results. This is a known
    /// limitation; schema validation should constrain value ranges where precision matters.
    fn scalar_eq(a: &AttributeValue, b: &AttributeValue) -> bool {
        match (a, b) {
            (AttributeValue::Integer(ai), AttributeValue::Integer(bi)) => ai == bi,
            (AttributeValue::Float(af), AttributeValue::Float(bf)) => af == bf,
            (AttributeValue::Integer(ai), AttributeValue::Float(bf)) => {
                OrderedFloat(*ai as f64) == *bf
            }
            (AttributeValue::Float(af), AttributeValue::Integer(bi)) => {
                *af == OrderedFloat(*bi as f64)
            }
            (AttributeValue::String(a), AttributeValue::String(b)) => a == b,
            (AttributeValue::Bool(a), AttributeValue::Bool(b)) => a == b,
            (AttributeValue::Timestamp(a), AttributeValue::Timestamp(b)) => a == b,
            _ => false,
        }
    }

    fn stack_scalar_eq(a: &StackValue, b: &StackValue) -> bool {
        match (a, b) {
            (StackValue::Integer(ai), StackValue::Integer(bi)) => ai == bi,
            (StackValue::Float(af), StackValue::Float(bf)) => af == bf,
            (StackValue::Integer(ai), StackValue::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
            (StackValue::Float(af), StackValue::Integer(bi)) => *af == OrderedFloat(*bi as f64),
            (StackValue::String(a), StackValue::String(b)) => a == b,
            (StackValue::Timestamp(a), StackValue::Timestamp(b)) => a == b,
            _ => false,
        }
    }


    /// Scalar ordering for use by `Lt`/`Lte`/`Gt`/`Gte`. Eliminates the four
    /// near-identical match blocks that would otherwise exist for each operator.
    ///
    /// Same int/float coercion caveat as `scalar_eq`.
    fn stack_scalar_cmp(a: &StackValue, b: &StackValue) -> Result<Ordering, String> {
        match (a, b) {
            (StackValue::Integer(ai), StackValue::Integer(bi)) => Ok(ai.cmp(bi)),
            (StackValue::Float(af), StackValue::Float(bf)) => Ok(af.cmp(bf)),
            (StackValue::Integer(ai), StackValue::Float(bf)) => {
                Ok(OrderedFloat(*ai as f64).cmp(bf))
            }
            (StackValue::Float(af), StackValue::Integer(bi)) => {
                Ok(af.cmp(&OrderedFloat(*bi as f64)))
            }
            (StackValue::String(a), StackValue::String(b)) => Ok(a.cmp(b)),
            (StackValue::Timestamp(a), StackValue::Timestamp(b)) => Ok(a.cmp(b)),
            _ => Err(format!("cannot order {:?} and {:?}", a, b)),
        }
    }

    /// Attribute value equality using `scalar_eq` for scalars (handles int/float coercion).
    /// Used by `ContainsAll` and `ContainsAny` instead of derived `PartialEq`.
    fn attribute_value_eq(a: &AttributeValue, b: &AttributeValue) -> bool {
        match (a, b) {
            (AttributeValue::EntityRef(ea), AttributeValue::EntityRef(eb)) => ea == eb,
            (AttributeValue::Integer(_), _)
            | (AttributeValue::Float(_), _)
            | (AttributeValue::String(_), _)
            | (AttributeValue::Bool(_), _)
            | (AttributeValue::Timestamp(_), _) => Self::scalar_eq(a, b),
            _ => false,
        }
    }

    fn stack_val_attribute_val_eq(a: &StackValue, b: &AttributeValue) -> bool {
        match (a, b) {
            (StackValue::Integer(ai), AttributeValue::Integer(bi)) => ai == bi,
            (StackValue::Float(af), AttributeValue::Float(bf)) => af == bf,
            (StackValue::Integer(ai), AttributeValue::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
            (StackValue::Float(af), AttributeValue::Integer(bi)) => *af == OrderedFloat(*bi as f64),
            (StackValue::String(as_val), AttributeValue::String(bs)) => as_val == bs,
            (StackValue::Bool(ab), AttributeValue::Bool(bb)) => ab == bb,
            (StackValue::Timestamp(at), AttributeValue::Timestamp(bt)) => at == bt,
            (StackValue::EntityRef(ae), AttributeValue::EntityRef(be)) => ae == be,
            (StackValue::IpAddr(ai), AttributeValue::IpAddr(bi)) => ai == bi,
            _ => false,
        }
    }

    /// Glob pattern matching: `*` matches any sequence of characters (including empty).
    ///
    /// Algorithm: split the pattern on `*`. The first segment must anchor to the
    /// start of the string; the last segment must anchor to the end. Middle segments
    /// must appear in left-to-right order somewhere in the middle.
    fn glob_match(s: &str, pattern: &str) -> bool {
        let parts: Vec<&str> = pattern.split('*').collect();

        // No wildcards: exact match only.
        if parts.len() == 1 {
            return s == pattern;
        }

        let prefix = parts[0];
        let suffix = parts[parts.len() - 1];

        if !s.starts_with(prefix) || !s.ends_with(suffix) {
            return false;
        }

        // Guard against overlapping prefix/suffix consuming more than the full string.
        if prefix.len() + suffix.len() > s.len() {
            return false;
        }

        // Scan the region between the anchored prefix and suffix for middle segments.
        let mut remaining = &s[prefix.len()..s.len() - suffix.len()];
        for part in &parts[1..parts.len() - 1] {
            if part.is_empty() {
                continue;
            }
            match remaining.find(part) {
                Some(pos) => remaining = &remaining[pos + part.len()..],
                None => return false,
            }
        }

        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use arbor_types::{
        flatten_attributes, AttributeNameId, AttributeValue, AttributeValueView, Attributes,
        EntityResolver, EntityTypeId, IndexedAttributeValue, IndexedEntity, SortedSetRef,
        VariableRef, VariableScope,
    };

    /// Shared by the test resolvers below: binary-searches into `arena`
    /// (the full shared arena -- `Object`'s `SortedSetRef` offsets are
    /// absolute into it, so a pre-sliced window would misindex on the
    /// second hop) starting at `base`, then follows `Object` hops the same
    /// way `Snapshot`'s real `resolve_attribute_path` does.
    fn resolve_path_in<'a>(
        arena: &'a [(AttributeNameId, IndexedAttributeValue)],
        base: SortedSetRef,
        path: &[AttributeNameId],
    ) -> Option<AttributeValueView<'a>> {
        if path.is_empty() {
            return None;
        }
        let pairs = &arena[base.offset as usize..(base.offset + base.len) as usize];
        let mut current = pairs
            .binary_search_by_key(&path[0], |(k, _)| *k)
            .ok()
            .map(|i| &pairs[i].1)?;
        for &name in &path[1..] {
            match current {
                IndexedAttributeValue::Object(nested) => {
                    let nested_pairs = &arena[nested.offset as usize..(nested.offset + nested.len) as usize];
                    current = nested_pairs
                        .binary_search_by_key(&name, |(k, _)| *k)
                        .ok()
                        .map(|i| &nested_pairs[i].1)?;
                }
                _ => return None,
            }
        }
        Some(current.as_view())
    }

    // ── Test infrastructure ───────────────────────────────────────────────────

    /// Resolver that always returns None/empty — for tests that don't use
    /// InHierarchy(Variable) or real attribute values (an entity built with
    /// `SortedSetRef::EMPTY` resolves to an empty slice regardless of which
    /// resolver backs it, so this is safe for those).
    struct NoopResolver;

    impl EntityResolver for NoopResolver {
        fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
        fn ancestors_of(&self, _: u32) -> Option<&[u32]> { None }
        fn resolve_attribute_path(&self, _: SortedSetRef, _: &[AttributeNameId]) -> Option<AttributeValueView<'_>> { None }
        fn attribute_set_values(&self, _: SortedSetRef) -> Vec<AttributeValueView<'_>> { Vec::new() }
        fn attribute_pairs_view(&self, _: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)> { Vec::new() }
    }

    /// Minimal resolver for tests that only need attribute resolution (no
    /// InHierarchy/entity-index lookups) -- holds the attribute arena a
    /// `make_entity_with_attr`-built entity's `SortedSetRef` points into.
    struct AttrResolver {
        pairs: Vec<(AttributeNameId, IndexedAttributeValue)>,
        values: Vec<IndexedAttributeValue>,
    }

    impl EntityResolver for AttrResolver {
        fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
        fn ancestors_of(&self, _: u32) -> Option<&[u32]> { None }
        fn resolve_attribute_path(&self, base: SortedSetRef, path: &[AttributeNameId]) -> Option<AttributeValueView<'_>> {
            resolve_path_in(&self.pairs, base, path)
        }
        fn attribute_set_values(&self, range: SortedSetRef) -> Vec<AttributeValueView<'_>> {
            self.values[range.offset as usize..(range.offset + range.len) as usize]
                .iter()
                .map(|v| v.as_view())
                .collect()
        }
        fn attribute_pairs_view(&self, range: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)> {
            self.pairs[range.offset as usize..(range.offset + range.len) as usize]
                .iter()
                .map(|(k, v)| (*k, v.as_view()))
                .collect()
        }
    }

    /// HashMap-based resolver — for InHierarchy tests that need entity lookups.
    /// Each entity carries its own ancestors `Vec` alongside it (test-only;
    /// production uses a single shared arena on `Snapshot`). Attribute arenas
    /// (`pairs`/`values`) are shared across all entities registered with one
    /// `MapResolver`, matching how `Snapshot` shares one arena per field.
    struct MapResolver {
        entities: HashMap<u32, (IndexedEntity, Vec<u32>)>,
        pairs: Vec<(AttributeNameId, IndexedAttributeValue)>,
        values: Vec<IndexedAttributeValue>,
    }

    impl MapResolver {
        fn new() -> Self {
            Self { entities: HashMap::new(), pairs: Vec::new(), values: Vec::new() }
        }
        fn insert(mut self, entity: IndexedEntity, ancestors: Vec<u32>) -> Self {
            self.entities.insert(entity.idx, (entity, ancestors));
            self
        }
        /// Flattens `attrs` into this resolver's shared arena and attaches
        /// the resulting `SortedSetRef` to the already-inserted entity `idx`.
        fn with_attributes(mut self, idx: u32, attrs: &Attributes) -> Self {
            let range = flatten_attributes(attrs, &mut self.pairs, &mut self.values);
            if let Some((entity, _)) = self.entities.get_mut(&idx) {
                entity.attributes = range;
            }
            self
        }
    }

    impl EntityResolver for MapResolver {
        fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
            self.entities.get(&index).map(|(e, _)| e)
        }
        fn ancestors_of(&self, index: u32) -> Option<&[u32]> {
            self.entities.get(&index).map(|(_, a)| a.as_slice())
        }
        fn resolve_attribute_path(&self, base: SortedSetRef, path: &[AttributeNameId]) -> Option<AttributeValueView<'_>> {
            resolve_path_in(&self.pairs, base, path)
        }
        fn attribute_set_values(&self, range: SortedSetRef) -> Vec<AttributeValueView<'_>> {
            self.values[range.offset as usize..(range.offset + range.len) as usize]
                .iter()
                .map(|v| v.as_view())
                .collect()
        }
        fn attribute_pairs_view(&self, range: SortedSetRef) -> Vec<(AttributeNameId, AttributeValueView<'_>)> {
            self.pairs[range.offset as usize..(range.offset + range.len) as usize]
                .iter()
                .map(|(k, v)| (*k, v.as_view()))
                .collect()
        }
    }

    // ── Entity helpers ────────────────────────────────────────────────────────

    fn make_test_entity() -> IndexedEntity {
        IndexedEntity {
            idx: 0,
            attributes: SortedSetRef::EMPTY,
            entity_type: EntityTypeId::new(1),
            ancestors: SortedSetRef::EMPTY,
            principal_of_policies: None,
            resource_of_policies: None,
            effective_principal_policies: None,
            effective_resource_policies: None,
        }
    }

    fn make_entity_at(idx: u32) -> IndexedEntity {
        IndexedEntity {
            idx,
            attributes: SortedSetRef::EMPTY,
            entity_type: EntityTypeId::new(1),
            ancestors: SortedSetRef::EMPTY,
            principal_of_policies: None,
            resource_of_policies: None,
            effective_principal_policies: None,
            effective_resource_policies: None,
        }
    }

    /// Returns the entity (with its `attributes` `SortedSetRef` already
    /// pointing into the returned resolver's arena) plus the `AttrResolver`
    /// it must be evaluated with.
    fn make_entity_with_attr(attr_id: u32, value: AttributeValue) -> (IndexedEntity, AttrResolver) {
        let mut attrs = Attributes::new();
        attrs.set(AttributeNameId::new(attr_id), value);
        let mut pairs = Vec::new();
        let mut values = Vec::new();
        let range = flatten_attributes(&attrs, &mut pairs, &mut values);
        let mut entity = make_test_entity();
        entity.attributes = range;
        (entity, AttrResolver { pairs, values })
    }

    /// Returns the entity (with an empty `ancestors` ref — this test resolver
    /// tracks ancestors separately, not via a shared arena) plus its sorted
    /// ancestor list for registering with `MapResolver::insert`.
    fn make_entity_with_ancestors(idx: u32, ancestors: &[u32]) -> (IndexedEntity, Vec<u32>) {
        let entity = make_entity_at(idx);
        let mut sorted = ancestors.to_vec();
        sorted.sort_unstable();
        (entity, sorted)
    }

    fn var_ref_principal(attr_id: u32) -> VariableRef {
        VariableRef {
            scope: VariableScope::Principal,
            path: vec![AttributeNameId::new(attr_id)],
        }
    }

    /// Returns a VariableRef for the scope entity itself (empty path).
    fn scope_var(scope: VariableScope) -> VariableRef {
        VariableRef { scope, path: vec![] }
    }

    // ===== Basic Comparison Tests =====

    #[test]
    fn test_simple_equality() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(42),
            OpCode::Eq,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_simple_inequality() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(43),
            OpCode::Eq,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_and_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Lt,
            OpCode::PushInteger(5),
            OpCode::PushInteger(5),
            OpCode::Eq,
            OpCode::And,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_or_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        // (10 > 20) OR (5 == 5) → false OR true → true
        let result = vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Gt,
            OpCode::PushInteger(5),
            OpCode::PushInteger(5),
            OpCode::Eq,
            OpCode::Or,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_not_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        // NOT(5 == 10) → NOT(false) → true
        let result = vm.evaluate(&[
            OpCode::PushInteger(5),
            OpCode::PushInteger(10),
            OpCode::Eq,
            OpCode::Not,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_comparison_operations() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Lt,
        ], &ctx), ConditionResult::True);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(20),
            OpCode::PushInteger(20),
            OpCode::Lte,
        ], &ctx), ConditionResult::True);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(30),
            OpCode::PushInteger(20),
            OpCode::Gt,
        ], &ctx), ConditionResult::True);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(20),
            OpCode::PushInteger(20),
            OpCode::Gte,
        ], &ctx), ConditionResult::True);
    }

    // ===== Missing Attribute Tests =====

    #[test]
    fn test_missing_eq_is_false() {
        let principal = make_test_entity(); // no attributes
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_neq_is_false() {
        // Security test: missing != "restricted" must be false, not true.
        // If this were true, `permit if principal.tier != "restricted"` would
        // grant access to any principal without a tier attribute.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("restricted".into()),
            OpCode::Neq,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_lt_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(10),
            OpCode::Lt,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_in_set_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushSet(vec![AttributeValue::String("admin".into())]),
            OpCode::In,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_not_is_invalid() {
        // Compiler invariant violation: PushVariable not consumed by a comparison.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::Not,
        ], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_bare_variable_on_stack_is_invalid() {
        // A lone PushVariable with nothing consuming it is a compiler bug.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::PushVariable(var_ref_principal(99))], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== HasAttribute Tests =====

    #[test]
    fn test_has_attribute_present() {
        let (principal, resolver) = make_entity_with_attr(1, AttributeValue::Bool(true));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::HasAttribute(var_ref_principal(1))], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_has_attribute_absent() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::HasAttribute(var_ref_principal(99))], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    // ===== Variable Resolution Tests =====

    #[test]
    fn test_variable_resolution_eq() {
        let (principal, resolver) = make_entity_with_attr(1, AttributeValue::String("gold".into()));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_missing_and_true_is_false() {
        // (true) AND (missing == "gold") → true AND false → false
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushBool(true),
            OpCode::PushBool(true),
            OpCode::Eq,
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
            OpCode::And,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    // ===== Set Operation Tests =====

    #[test]
    fn test_contains_all_int_float_coercion() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushSet(vec![
                AttributeValue::Integer(5),
                AttributeValue::Integer(10),
            ]),
            OpCode::PushSet(vec![
                AttributeValue::Float(ordered_float::OrderedFloat(5.0)),
            ]),
            OpCode::ContainsAll,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_push_set_in_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("editor".into()),
            OpCode::PushSet(vec![
                AttributeValue::String("admin".into()),
                AttributeValue::String("editor".into()),
            ]),
            OpCode::In,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_neq_on_equal_values_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(42),
            OpCode::Neq,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_neq_on_unequal_values_is_true() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(99),
            OpCode::Neq,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== String Operation Tests =====

    #[test]
    fn test_starts_with_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello".into()),
            OpCode::StartsWith,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_starts_with_no_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("world".into()),
            OpCode::StartsWith,
        ], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_ends_with_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("world".into()),
            OpCode::EndsWith,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_string_contains_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("lo wo".into()),
            OpCode::StringContains,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_string_ops_missing_is_false() {
        let principal = make_test_entity(); // no attributes
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("prefix".into()),
            OpCode::StartsWith,
        ], &ctx), ConditionResult::False);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("suffix".into()),
            OpCode::EndsWith,
        ], &ctx), ConditionResult::False);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("needle".into()),
            OpCode::StringContains,
        ], &ctx), ConditionResult::False);

        let mut vm = BytecodeVM::new();
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("*".into()),
            OpCode::Like,
        ], &ctx), ConditionResult::False);
    }

    // ===== Like / Glob Tests =====

    #[test]
    fn test_like_exact_match() {
        assert!(BytecodeVM::glob_match("hello", "hello"));
        assert!(!BytecodeVM::glob_match("hello", "hell"));
        assert!(!BytecodeVM::glob_match("hell", "hello"));
    }

    #[test]
    fn test_like_star_matches_all() {
        assert!(BytecodeVM::glob_match("anything", "*"));
        assert!(BytecodeVM::glob_match("", "*"));
    }

    #[test]
    fn test_like_prefix_star() {
        assert!(BytecodeVM::glob_match("foobar", "foo*"));
        assert!(BytecodeVM::glob_match("foo", "foo*")); // * matches empty
        assert!(!BytecodeVM::glob_match("barfoo", "foo*"));
    }

    #[test]
    fn test_like_star_suffix() {
        assert!(BytecodeVM::glob_match("foobar", "*bar"));
        assert!(BytecodeVM::glob_match("bar", "*bar")); // * matches empty
        assert!(!BytecodeVM::glob_match("foobar", "*foo"));
    }

    #[test]
    fn test_like_infix_star() {
        assert!(BytecodeVM::glob_match("fooXbar", "foo*bar"));
        assert!(BytecodeVM::glob_match("foobar", "foo*bar")); // * matches empty
        assert!(!BytecodeVM::glob_match("fooXbaz", "foo*bar"));
        assert!(!BytecodeVM::glob_match("foobarbaz", "foo*bar"));
    }

    #[test]
    fn test_like_multiple_stars() {
        assert!(BytecodeVM::glob_match("fooXbarYbaz", "foo*bar*baz"));
        assert!(BytecodeVM::glob_match("foobarbaz", "foo*bar*baz")); // stars match empty
        assert!(!BytecodeVM::glob_match("foobarbaz", "foo*bar*qux"));
    }

    #[test]
    fn test_like_opcode() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello*".into()),
            OpCode::Like,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== IsType Tests =====

    #[test]
    fn test_is_type_match() {
        let mut principal = make_test_entity();
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Principal, EntityTypeId::new(42))], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_is_type_no_match() {
        let mut principal = make_test_entity();
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Principal, EntityTypeId::new(99))], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_is_type_resource() {
        let principal = make_test_entity();
        let mut resource = make_test_entity();
        resource.entity_type = EntityTypeId::new(7);
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Resource, EntityTypeId::new(7))], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_is_type_context_scope_is_invalid() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Context, EntityTypeId::new(1))], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== InHierarchy Tests =====
    //
    // InHierarchy(descendant, ancestor) where both are ResolvedEntityIndex:
    //   Variable(scope_var) → resolve_variable(scope, []) → EntityRef(entity.idx)
    //                       → get_entity(idx) → check ancestors.contains(anc_idx)
    //   Direct(idx)         → use idx directly → get_entity(idx)
    //
    // The entity under test must be in the MapResolver at its own idx.

    #[test]
    fn test_in_hierarchy_self_inclusive() {
        // Principal at idx=0, ancestors=[5]. InHierarchy(Variable(Principal), Direct(5)) → true.
        let (principal, principal_ancestors) = make_entity_with_ancestors(0, &[5]);
        let resource = make_entity_at(1);
        let resolver = MapResolver::new().insert(principal.clone(), principal_ancestors);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(scope_var(VariableScope::Principal)),
            ResolvedEntityIndex::Direct(5),
        )], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_ancestor_match() {
        // Principal has ancestors [10, 20, 30]. Check InHierarchy(principal, 20) → true.
        let (principal, principal_ancestors) = make_entity_with_ancestors(0, &[10, 20, 30]);
        let resource = make_entity_at(1);
        let resolver = MapResolver::new().insert(principal.clone(), principal_ancestors);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(scope_var(VariableScope::Principal)),
            ResolvedEntityIndex::Direct(20),
        )], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_no_match() {
        let (principal, principal_ancestors) = make_entity_with_ancestors(0, &[10, 20]);
        let resource = make_entity_at(1);
        let resolver = MapResolver::new().insert(principal.clone(), principal_ancestors);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(scope_var(VariableScope::Principal)),
            ResolvedEntityIndex::Direct(99),
        )], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_resource_scope() {
        // Resource at idx=1, ancestors=[7]. InHierarchy(Variable(Resource), Direct(7)) → true.
        let principal = make_entity_at(0);
        let (resource, resource_ancestors) = make_entity_with_ancestors(1, &[7]);
        let resolver = MapResolver::new().insert(resource.clone(), resource_ancestors);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(scope_var(VariableScope::Resource)),
            ResolvedEntityIndex::Direct(7),
        )], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_context_scope_is_invalid() {
        // Context scope with empty path: resolve_variable returns None → Err → Invalid.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(scope_var(VariableScope::Context)),
            ResolvedEntityIndex::Direct(1),
        )], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_in_hierarchy_combined_with_type_check() {
        // `permit if principal is Admin AND principal in AdminGroup`
        let (mut principal, principal_ancestors) = make_entity_with_ancestors(0, &[100]);
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_entity_at(1);
        let resolver = MapResolver::new().insert(principal.clone(), principal_ancestors);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[
            OpCode::IsType(VariableScope::Principal, EntityTypeId::new(42)),
            OpCode::InHierarchy(
                ResolvedEntityIndex::Variable(scope_var(VariableScope::Principal)),
                ResolvedEntityIndex::Direct(100),
            ),
            OpCode::And,
        ], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== InHierarchy(Variable(attr_path), Direct) Tests =====
    //
    // When left operand is a Variable with a non-empty path, the VM resolves the
    // attribute to an EntityRef(u32) index, then looks up that entity in the resolver.

    #[test]
    fn test_in_hierarchy_attr_match() {
        // principal.manager_ref = EntityRef(10u32)
        // manager entity at index 10, ancestors [10, 50] (self + AdminGroup)
        // InHierarchy(Variable(principal.manager_ref), Direct(50)) → true
        let (manager, manager_ancestors) = make_entity_with_ancestors(10, &[10, 50]);
        let mut attrs = Attributes::new();
        attrs.set(AttributeNameId::new(1), AttributeValue::EntityRef(10u32));
        let resolver = MapResolver::new()
            .insert(manager, manager_ancestors)
            .insert(make_entity_at(0), vec![])
            .with_attributes(0, &attrs);
        let principal = resolver.get_entity(0).unwrap().clone();
        let resource = make_entity_at(1);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(var_ref_principal(1)),
            ResolvedEntityIndex::Direct(50),
        )], &ctx);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_attr_no_match() {
        // manager at index 10, only self in ancestors — not in group 50
        let (manager, manager_ancestors) = make_entity_with_ancestors(10, &[10]);
        let mut attrs = Attributes::new();
        attrs.set(AttributeNameId::new(1), AttributeValue::EntityRef(10u32));
        let resolver = MapResolver::new()
            .insert(manager, manager_ancestors)
            .insert(make_entity_at(0), vec![])
            .with_attributes(0, &attrs);
        let principal = resolver.get_entity(0).unwrap().clone();
        let resource = make_entity_at(1);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(var_ref_principal(1)),
            ResolvedEntityIndex::Direct(50),
        )], &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_attr_missing_attribute_is_invalid() {
        // principal has no attr at id 1 → resolve_variable returns None
        // → pattern `Some(EntityRef(ent)) = None` fails → Err → Invalid
        let principal = make_test_entity();
        let resource = make_entity_at(1);
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(var_ref_principal(1)),
            ResolvedEntityIndex::Direct(50),
        )], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_in_hierarchy_attr_unknown_index_is_invalid() {
        // Attribute holds index 999, which is not present in the resolver.
        // get_entity(999) returns None → Err → Invalid.
        // AttrResolver's get_entity always returns None, which is exactly
        // what this test needs (999 unregistered) -- no MapResolver required.
        let (principal, resolver) = make_entity_with_attr(1, AttributeValue::EntityRef(999u32));
        let resource = make_entity_at(1);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(var_ref_principal(1)),
            ResolvedEntityIndex::Direct(50),
        )], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_in_hierarchy_attr_wrong_type_is_invalid() {
        // Attribute holds a String, not an EntityRef → pattern match fails → Invalid.
        let (principal, resolver) = make_entity_with_attr(1, AttributeValue::String("not-an-entity".into()));
        let resource = make_entity_at(1);
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();
        let result = vm.evaluate(&[OpCode::InHierarchy(
            ResolvedEntityIndex::Variable(var_ref_principal(1)),
            ResolvedEntityIndex::Direct(50),
        )], &ctx);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== Short-Circuit Tests =====

    #[test]
    fn test_short_circuit_and_with_missing() {
        let principal = make_test_entity(); // has nothing
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut vm = BytecodeVM::new();

        let instructions = vec![
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(1),
            OpCode::Eq,              // → false
            OpCode::JumpIfFalse(8),  // jump taken
            OpCode::PushVariable(var_ref_principal(2)),
            OpCode::PushInteger(2),
            OpCode::Eq,
            OpCode::Jump(9),
            OpCode::PushBool(false),
        ];

        let result = vm.evaluate(&instructions, &ctx);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_short_circuit_or_with_missing() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
        let mut _vm_placeholder = BytecodeVM::new();

        let (principal, resolver) = make_entity_with_attr(2, AttributeValue::String("value".into()));
        let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
        let mut vm = BytecodeVM::new();

        let instructions = vec![
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(1),
            OpCode::Eq,              // → false
            OpCode::JumpIfTrue(8),   // jump NOT taken
            OpCode::PushVariable(var_ref_principal(2)),
            OpCode::PushString("value".into()),
            OpCode::Eq,              // → true
            OpCode::Jump(9),
            OpCode::PushBool(true),
        ];

        let result = vm.evaluate(&instructions, &ctx);
        assert_eq!(result, ConditionResult::True);
    }
}
