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
    AttributeValue, ConditionResult, EntityTypeId, EvaluationContext, EvaluationError,
    OpCode, VariableRef, VariableScope,
};
use chrono::{DateTime, Utc};
use ordered_float::OrderedFloat;
use std::cmp::Ordering;
use std::net::IpAddr;
use uuid::Uuid;

#[derive(Debug, Clone, PartialEq)]
pub enum StackValue {
    Integer(i64),
    Float(OrderedFloat<f64>),
    Timestamp(DateTime<Utc>),
    String(String),
    Bool(bool),
    EntityRef(Uuid),
    IpAddr(IpAddr),
    Set(Vec<AttributeValue>),
    Missing,
}

/// Bytecode VM for condition evaluation
pub struct BytecodeVM<'a> {
    stack: Vec<StackValue>,
    context: &'a EvaluationContext<'a>,
}

impl<'a> BytecodeVM<'a> {
    pub fn new(context: &'a EvaluationContext<'a>) -> Self {
        Self {
            stack: Vec::with_capacity(16),
            context,
        }
    }

    /// Evaluate a compiled condition.
    ///
    /// Returns `True`/`False` on success, or `Invalid` on type errors or
    /// compiler invariant violations. Missing attributes evaluate to `false`
    /// at comparison boundaries; they are not errors.
    pub fn evaluate(&mut self, instructions: &[OpCode]) -> ConditionResult {
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
                    if let Err(e) = self.execute_instruction(other) {
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


    fn execute_instruction(&mut self, instruction: &OpCode) -> Result<(), String> {
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
            OpCode::PushEntityRef(uuid) => {
                self.stack.push(StackValue::EntityRef(*uuid));
                Ok(())
            }
            OpCode::PushVariable(var_ref) => self.execute_push_variable(var_ref),
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
            OpCode::HasAttribute(var_ref) => self.execute_has_attribute(var_ref),
            OpCode::StartsWith => self.execute_starts_with(),
            OpCode::EndsWith => self.execute_ends_with(),
            OpCode::StringContains => self.execute_string_contains(),
            OpCode::Like => self.execute_like(),
            OpCode::IsType(scope, type_id) => self.execute_is_type(scope, type_id),
            OpCode::InHierarchy(scope, target_idx) => self.execute_in_hierarchy(scope, *target_idx),
            OpCode::InHierarchyVar(var_ref, target_idx) => self.execute_in_hierarchy_var(var_ref, *target_idx),
            OpCode::ContainsInHierarchy(target_idx) => self.execute_contains_in_hierarchy(*target_idx),
            OpCode::JumpIfFalse(_) | OpCode::Jump(_) | OpCode::JumpIfTrue(_) => {
                Err("control flow shouldn't evaluate here".into())
            }
        }
    }

    // ===== Stack Operations =====

    fn execute_push_variable(&mut self, var_ref: &VariableRef) -> Result<(), String> {
        let value = match self.resolve_variable(var_ref) {
            Some(AttributeValue::String(s)) => StackValue::String(s),
            Some(AttributeValue::Float(f)) => StackValue::Float(f),
            Some(AttributeValue::Integer(i)) => StackValue::Integer(i),
            Some(AttributeValue::Bool(b)) => StackValue::Bool(b),
            Some(AttributeValue::IpAddr(ip)) => StackValue::IpAddr(ip),
            Some(AttributeValue::Timestamp(t)) => StackValue::Timestamp(t),
            Some(AttributeValue::EntityRef(u)) => StackValue::EntityRef(u),
            Some(AttributeValue::Set(s)) => StackValue::Set(s),
            // Objects cannot be directly compared; treat as Missing.
            Some(AttributeValue::Object(_)) | None => StackValue::Missing,
        };
        self.stack.push(value);
        Ok(())
    }

    /// Resolve an attribute path to its value. Returns `None` if the path does
    /// not exist or if context attributes are absent for a Context-scoped variable.
    fn resolve_variable(&self, var_ref: &VariableRef) -> Option<AttributeValue> {
        let base = match var_ref.scope {
            VariableScope::Principal => &self.context.principal.attributes,
            VariableScope::Resource => &self.context.resource.attributes,
            VariableScope::Context => self.context.context_attrs?,
        };
        base.get_nested(&var_ref.path).cloned()
    }

    // ===== Attribute Operations =====

    fn execute_has_attribute(&mut self, var_ref: &VariableRef) -> Result<(), String> {
        let exists = self.resolve_variable(var_ref).is_some();
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
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Set(set_vals), StackValue::Set(subset_vals)) => subset_vals
                .iter()
                .all(|sub| set_vals.iter().any(|s| Self::attribute_value_eq(s, sub))),
            _ => return Err("ContainsAll requires (set, set)".into()),
        };
        self.stack.push(StackValue::Bool(result));
        Ok(())
    }

    fn execute_contains_any(&mut self) -> Result<(), String> {
        let subset = self.pop()?;
        let set = self.pop()?;
        let result = match (set, subset) {
            (StackValue::Missing, _) | (_, StackValue::Missing) => false,
            (StackValue::Set(set_vals), StackValue::Set(subset_vals)) => subset_vals
                .iter()
                .any(|sub| set_vals.iter().any(|s| Self::attribute_value_eq(s, sub))),
            _ => return Err("ContainsAny requires (set, set)".into()),
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

    /// Checks whether the entity at `scope` is or descends from `target_idx`.
    ///
    /// The target index is pre-resolved by the compiler — UUID→index lookup happens
    /// at compile/index time, never at evaluation time. The VM performs a single
    /// roaring bitmap `contains` call, which is O(log n) worst-case and typically O(1).
    ///
    /// Self-inclusive: the snapshot builder is responsible for including each
    /// entity's own index in its `ancestors` bitmap.
    fn execute_in_hierarchy(&mut self, scope: &VariableScope, target_idx: u32) -> Result<(), String> {
        let entity = match scope {
            VariableScope::Principal => self.context.principal,
            VariableScope::Resource => self.context.resource,
            VariableScope::Context => return Err("InHierarchy is not valid on Context scope".into()),
        };
        self.stack.push(StackValue::Bool(entity.ancestors.contains(target_idx)));
        Ok(())
    }

    /// Resolves `var_ref` to an `EntityRef` attribute, looks up that entity in the
    /// snapshot via `EntityResolver`, and checks its `ancestors` bitmap for `target_idx`.
    ///
    /// Missing attribute or unresolvable UUID → `false` (treat like any missing value).
    /// Attribute is not an `EntityRef` → `Invalid` (type mismatch, compiler bug).
    /// No `EntityResolver` attached to context → `Invalid` (caller must use `with_entities`).
    fn execute_in_hierarchy_var(&mut self, var_ref: &VariableRef, target_idx: u32) -> Result<(), String> {
        let resolver = self.context.entities.ok_or(
            "InHierarchyVar: EvaluationContext has no EntityResolver — call with_entities(snapshot)"
        )?;

        let uuid = match self.resolve_variable(var_ref) {
            None => {
                // Missing attribute → false, consistent with other Missing semantics.
                self.stack.push(StackValue::Bool(false));
                return Ok(());
            }
            Some(AttributeValue::EntityRef(uuid)) => uuid,
            Some(other) => {
                return Err(format!(
                    "InHierarchyVar: expected EntityRef attribute, got {:?}",
                    other
                ));
            }
        };

        let entity_idx = match resolver.resolve_uuid(&uuid) {
            Some(idx) => idx,
            None => {
                // UUID not in snapshot (stale reference) → false.
                self.stack.push(StackValue::Bool(false));
                return Ok(());
            }
        };

        let entity = match resolver.get_entity(entity_idx) {
            Some(e) => e,
            None => {
                self.stack.push(StackValue::Bool(false));
                return Ok(());
            }
        };

        self.stack.push(StackValue::Bool(entity.ancestors.contains(target_idx)));
        Ok(())
    }

    /// Pops a set of `EntityRef`s and checks whether any element is the target or
    /// a descendant of it. Short-circuits on the first match.
    ///
    /// Unresolvable UUIDs are skipped — they are simply not in the hierarchy.
    /// Non-`EntityRef` elements return `Invalid` (compiler should never produce this).
    fn execute_contains_in_hierarchy(&mut self, target_idx: u32) -> Result<(), String> {
        let resolver = self.context.entities.ok_or(
            "ContainsInHierarchy: EvaluationContext has no EntityResolver — call with_entities(snapshot)"
        )?;

        let set = match self.pop()? {
            StackValue::Missing => {
                self.stack.push(StackValue::Bool(false));
                return Ok(());
            }
            StackValue::Set(vals) => vals,
            other => return Err(format!("ContainsInHierarchy requires a set, got {:?}", other)),
        };

        for elem in &set {
            match elem {
                AttributeValue::EntityRef(uuid) => {
                    // Unresolvable UUID → not in hierarchy, skip.
                    let Some(entity_idx) = resolver.resolve_uuid(uuid) else { continue };
                    let Some(entity) = resolver.get_entity(entity_idx) else { continue };
                    if entity.ancestors.contains(target_idx) {
                        self.stack.push(StackValue::Bool(true));
                        return Ok(());
                    }
                }
                other => {
                    return Err(format!(
                        "ContainsInHierarchy: set element must be EntityRef, got {:?}",
                        other
                    ));
                }
            }
        }

        self.stack.push(StackValue::Bool(false));
        Ok(())
    }

    /// Pushes `Bool(true)` if the entity at `scope` has `entity_type == type_id`.
    /// Never produces Missing. The entity is always present in the evaluation context.
    fn execute_is_type(&mut self, scope: &VariableScope, type_id: &EntityTypeId) -> Result<(), String> {
        let entity_type = match scope {
            VariableScope::Principal => self.context.principal.entity_type,
            VariableScope::Resource => self.context.resource.entity_type,
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
    use arbor_types::{
        AttributeNameId, AttributeValue, Attributes, EntityTypeId, IndexedEntity,
        VariableRef, VariableScope,
    };
    use roaring::RoaringBitmap;

    fn make_test_entity() -> IndexedEntity {
        IndexedEntity {
            attributes: Attributes::new(),
            entity_type: EntityTypeId::new(1),
            descendants: RoaringBitmap::new(),
            ancestors: RoaringBitmap::new(),
            principal_of_policies: None,
            resource_of_policies: None,
        }
    }

    fn make_entity_with_attr(attr_id: u32, value: AttributeValue) -> IndexedEntity {
        let mut entity = make_test_entity();
        entity.attributes.set(AttributeNameId::new(attr_id), value);
        entity
    }

    fn var_ref_principal(attr_id: u32) -> VariableRef {
        VariableRef {
            scope: VariableScope::Principal,
            path: vec![AttributeNameId::new(attr_id)],
        }
    }

    // ===== Existing Tests =====

    #[test]
    fn test_simple_equality() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(42),
            OpCode::Eq,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_simple_inequality() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(43),
            OpCode::Eq,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_and_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Lt,
            OpCode::PushInteger(5),
            OpCode::PushInteger(5),
            OpCode::Eq,
            OpCode::And,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_or_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        // (10 > 20) OR (5 == 5) → false OR true → true
        let result = vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Gt,
            OpCode::PushInteger(5),
            OpCode::PushInteger(5),
            OpCode::Eq,
            OpCode::Or,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_not_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        // NOT(5 == 10) → NOT(false) → true
        let result = vm.evaluate(&[
            OpCode::PushInteger(5),
            OpCode::PushInteger(10),
            OpCode::Eq,
            OpCode::Not,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_comparison_operations() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(10),
            OpCode::PushInteger(20),
            OpCode::Lt,
        ]), ConditionResult::True);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(20),
            OpCode::PushInteger(20),
            OpCode::Lte,
        ]), ConditionResult::True);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(30),
            OpCode::PushInteger(20),
            OpCode::Gt,
        ]), ConditionResult::True);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushInteger(20),
            OpCode::PushInteger(20),
            OpCode::Gte,
        ]), ConditionResult::True);
    }

    // ===== Missing Attribute Tests =====

    #[test]
    fn test_missing_eq_is_false() {
        let principal = make_test_entity(); // no attributes
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_neq_is_false() {
        // Security test: missing != "restricted" must be false, not true.
        // If this were true, `permit if principal.tier != "restricted"` would
        // grant access to any principal without a tier attribute.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("restricted".into()),
            OpCode::Neq,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_lt_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(10),
            OpCode::Lt,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_in_set_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushSet(vec![ AttributeValue::String("admin".into( ))]),
            OpCode::In,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_missing_not_is_invalid() {
        // Compiler invariant violation: PushVariable not consumed by a comparison.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::Not,
        ]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_bare_variable_on_stack_is_invalid() {
        // A lone PushVariable with nothing consuming it is a compiler bug.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::PushVariable(var_ref_principal(99))]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== HasAttribute Tests =====

    #[test]
    fn test_has_attribute_present() {
        let principal = make_entity_with_attr(1,  AttributeValue::Bool(true ));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::HasAttribute(var_ref_principal(1))]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_has_attribute_absent() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::HasAttribute(var_ref_principal(99))]);
        assert_eq!(result, ConditionResult::False);
    }

    // ===== Variable Resolution Tests =====

    #[test]
    fn test_variable_resolution_eq() {
        let principal = make_entity_with_attr(
            1,
             AttributeValue::String("gold".into( )),
        );
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_missing_and_true_is_false() {
        // (true) AND (missing == "gold") → true AND false → false
        // Verifies Missing is consumed by Eq before And sees it.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushBool(true),
            OpCode::PushBool(true),
            OpCode::Eq,
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("gold".into()),
            OpCode::Eq,
            OpCode::And,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    // ===== Set Operation Tests =====

    #[test]
    fn test_contains_all_int_float_coercion() {
        // Set contains Integer(5); subset has Float(5.0). ContainsAll must use
        // scalar_eq (which handles coercion), not derived PartialEq.
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushSet(vec![
                 AttributeValue::Integer(5 ),
                 AttributeValue::Integer(10 ),
            ]),
            OpCode::PushSet(vec![
                 AttributeValue::Float(ordered_float::OrderedFloat(5.0 )),
            ]),
            OpCode::ContainsAll,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_push_set_in_operation() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("editor".into()),
            OpCode::PushSet(vec![
                 AttributeValue::String("admin".into( )),
                 AttributeValue::String("editor".into( )),
            ]),
            OpCode::In,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_neq_on_equal_values_is_false() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(42),
            OpCode::Neq,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_neq_on_unequal_values_is_true() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushInteger(42),
            OpCode::PushInteger(99),
            OpCode::Neq,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== String Operation Tests =====

    #[test]
    fn test_starts_with_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello".into()),
            OpCode::StartsWith,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_starts_with_no_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("world".into()),
            OpCode::StartsWith,
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_ends_with_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("world".into()),
            OpCode::EndsWith,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_string_contains_match() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("lo wo".into()),
            OpCode::StringContains,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_string_ops_missing_is_false() {
        let principal = make_test_entity(); // no attributes
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("prefix".into()),
            OpCode::StartsWith,
        ]), ConditionResult::False);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("suffix".into()),
            OpCode::EndsWith,
        ]), ConditionResult::False);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("needle".into()),
            OpCode::StringContains,
        ]), ConditionResult::False);

        let mut vm = BytecodeVM::new(&ctx);
        assert_eq!(vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushString("*".into()),
            OpCode::Like,
        ]), ConditionResult::False);
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
        // Pattern ending doesn't anchor: "foobar..." won't match "foo*bar" if trailing chars remain
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
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello*".into()),
            OpCode::Like,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== IsType Tests =====

    #[test]
    fn test_is_type_match() {
        let mut principal = make_test_entity();
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Principal, EntityTypeId::new(42))]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_is_type_no_match() {
        let mut principal = make_test_entity();
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Principal, EntityTypeId::new(99))]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_is_type_resource() {
        let principal = make_test_entity();
        let mut resource = make_test_entity();
        resource.entity_type = EntityTypeId::new(7);
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Resource, EntityTypeId::new(7))]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_is_type_context_scope_is_invalid() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::IsType(VariableScope::Context, EntityTypeId::new(1))]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== InHierarchy Tests =====

    fn make_entity_with_ancestors(indices: &[u32]) -> IndexedEntity {
        let mut entity = make_test_entity();
        for &idx in indices {
            entity.ancestors.insert(idx);
        }
        entity
    }

    #[test]
    fn test_in_hierarchy_self_inclusive() {
        // The snapshot builder includes the entity's own index in ancestors.
        // Index 5 is in the ancestors bitmap → InHierarchy(Principal, 5) → true.
        let principal = make_entity_with_ancestors(&[5]);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchy(VariableScope::Principal, 5)]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_ancestor_match() {
        // Principal has ancestors [10, 20, 30] (e.g., member of groups at those indices).
        let principal = make_entity_with_ancestors(&[10, 20, 30]);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchy(VariableScope::Principal, 20)]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_no_match() {
        let principal = make_entity_with_ancestors(&[10, 20]);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchy(VariableScope::Principal, 99)]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_resource_scope() {
        let principal = make_test_entity();
        let resource = make_entity_with_ancestors(&[7]);
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchy(VariableScope::Resource, 7)]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_context_scope_is_invalid() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchy(VariableScope::Context, 1)]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_in_hierarchy_combined_with_type_check() {
        // `permit if principal is Admin AND principal in AdminGroup`
        // Principal is type 42, has ancestor index 100.
        let mut principal = make_entity_with_ancestors(&[100]);
        principal.entity_type = EntityTypeId::new(42);
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::IsType(VariableScope::Principal, EntityTypeId::new(42)),
            OpCode::InHierarchy(VariableScope::Principal, 100),
            OpCode::And,
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    // ===== InHierarchyVar Tests =====

    use std::collections::HashMap;
    use arbor_types::EntityResolver;

    struct TestEntityResolver {
        uuid_map: HashMap<Uuid, u32>,
        entities: HashMap<u32, IndexedEntity>,
    }

    impl TestEntityResolver {
        fn new() -> Self {
            Self { uuid_map: HashMap::new(), entities: HashMap::new() }
        }

        fn add(&mut self, uuid: Uuid, idx: u32, ancestors: &[u32]) {
            let mut entity = make_test_entity();
            for &a in ancestors {
                entity.ancestors.insert(a);
            }
            self.uuid_map.insert(uuid, idx);
            self.entities.insert(idx, entity);
        }
    }

    impl EntityResolver for TestEntityResolver {
        fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
            self.entities.get(&index)
        }
        fn resolve_uuid(&self, uuid: &Uuid) -> Option<u32> {
            self.uuid_map.get(uuid).copied()
        }
    }

    #[test]
    fn test_in_hierarchy_var_match() {
        // principal.manager = EntityRef(manager_uuid)
        // manager is at index 10, has ancestor 50 (AdminGroup)
        // InHierarchyVar(principal.manager, 50) → true
        let manager_uuid = Uuid::new_v4();
        let mut store = TestEntityResolver::new();
        store.add(manager_uuid, 10, &[10, 50]); // self-inclusive + AdminGroup

        let principal = make_entity_with_attr(
            1,
            AttributeValue::EntityRef(manager_uuid),
        );
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None)
            .with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::InHierarchyVar(var_ref_principal(1), 50),
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_in_hierarchy_var_no_match() {
        let manager_uuid = Uuid::new_v4();
        let mut store = TestEntityResolver::new();
        store.add(manager_uuid, 10, &[10]); // only self, not in group 50

        let principal = make_entity_with_attr(1, AttributeValue::EntityRef(manager_uuid));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchyVar(var_ref_principal(1), 50)]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_var_missing_attribute_is_false() {
        // principal has no attr at id 1 → Missing → false
        let store = TestEntityResolver::new();
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchyVar(var_ref_principal(1), 50)]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_var_stale_uuid_is_false() {
        // Attribute holds a UUID not present in the snapshot → false
        let store = TestEntityResolver::new(); // empty — UUID won't resolve
        let principal = make_entity_with_attr(1, AttributeValue::EntityRef(Uuid::new_v4()));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchyVar(var_ref_principal(1), 50)]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_in_hierarchy_var_wrong_type_is_invalid() {
        // Attribute holds a String, not an EntityRef → type mismatch → Invalid
        let store = TestEntityResolver::new();
        let principal = make_entity_with_attr(
            1,
             AttributeValue::String("not-an-entity".into( )),
        );
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchyVar(var_ref_principal(1), 50)]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_in_hierarchy_var_no_resolver_is_invalid() {
        // No EntityResolver attached → Invalid
        let principal = make_entity_with_attr(1, AttributeValue::EntityRef(Uuid::new_v4()));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None); // no with_entities
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[OpCode::InHierarchyVar(var_ref_principal(1), 50)]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    // ===== ContainsInHierarchy Tests =====

    #[test]
    fn test_contains_in_hierarchy_match() {
        // principal.groups = [group_a_uuid, group_b_uuid]
        // group_b is at index 20, has ancestor 50 (AdminGroup)
        // ContainsInHierarchy(50) → true (group_b is in AdminGroup hierarchy)
        let group_a = Uuid::new_v4();
        let group_b = Uuid::new_v4();
        let mut store = TestEntityResolver::new();
        store.add(group_a, 10, &[10]);       // group_a — not in admin hierarchy
        store.add(group_b, 20, &[20, 50]);   // group_b — is in admin hierarchy

        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![
            AttributeValue::EntityRef(group_a),
            AttributeValue::EntityRef(group_b),
        ]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_contains_in_hierarchy_no_match() {
        // Neither group is in the target hierarchy
        let group_a = Uuid::new_v4();
        let group_b = Uuid::new_v4();
        let mut store = TestEntityResolver::new();
        store.add(group_a, 10, &[10]);
        store.add(group_b, 20, &[20]);

        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![
            AttributeValue::EntityRef(group_a),
            AttributeValue::EntityRef(group_b),
        ]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_contains_in_hierarchy_empty_set_is_false() {
        let store = TestEntityResolver::new();
        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_contains_in_hierarchy_missing_attribute_is_false() {
        let store = TestEntityResolver::new();
        let principal = make_test_entity(); // no groups attribute
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_contains_in_hierarchy_stale_uuid_skipped() {
        // One UUID resolves, one doesn't. Stale one is skipped; valid one is checked.
        let known = Uuid::new_v4();
        let stale = Uuid::new_v4();
        let mut store = TestEntityResolver::new();
        store.add(known, 10, &[10, 50]); // known is in hierarchy

        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![
            AttributeValue::EntityRef(stale),  // not in snapshot
            AttributeValue::EntityRef(known),  // is in hierarchy
        ]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert_eq!(result, ConditionResult::True);
    }

    #[test]
    fn test_contains_in_hierarchy_non_entity_ref_is_invalid() {
        let store = TestEntityResolver::new();
        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![
             AttributeValue::String("not-an-entity".into( )),
        ]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None).with_entities(&store);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_contains_in_hierarchy_no_resolver_is_invalid() {
        let principal = make_entity_with_attr(1, AttributeValue::Set(vec![
            AttributeValue::EntityRef(Uuid::new_v4()),
        ]));
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);
        let result = vm.evaluate(&[
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::ContainsInHierarchy(50),
        ]);
        assert!(matches!(result, ConditionResult::Invalid(_)));
    }

    #[test]
    fn test_short_circuit_and_with_missing() {
        let principal = make_test_entity(); // has nothing
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);

        // Instructions:
        // 0: PushVariable(1)
        // 1: PushScalar(1)
        // 2: Eq -> false
        // 3: JumpIfFalse(8)
        // 4: PushVariable(2)
        // 5: PushScalar(2)
        // 6: Eq
        // 7: Jump(9)
        // 8: PushScalar(false)
        let instructions = vec![
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(1),
            OpCode::Eq,              // -> false
            OpCode::JumpIfFalse(8),  // jump taken
            OpCode::PushVariable(var_ref_principal(2)),
            OpCode::PushInteger(2),
            OpCode::Eq,
            OpCode::Jump(9),
            OpCode::PushBool(false),
        ];

        let result = vm.evaluate(&instructions);
        assert_eq!(result, ConditionResult::False);
    }

    #[test]
    fn test_short_circuit_or_with_missing() {
        let principal = make_test_entity();
        let resource = make_test_entity();
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut _vm_placeholder = BytecodeVM::new(&ctx);

        // (principal.missing == 1) || (principal.present == "value")
        // principal.present exists.
        let principal = make_entity_with_attr(2,  AttributeValue::String("value".into( )));
        let ctx = EvaluationContext::new(&principal, &resource, None);
        let mut vm = BytecodeVM::new(&ctx);

        // Instructions:
        // 0: PushVariable(1)
        // 1: PushScalar(1)
        // 2: Eq -> false
        // 3: JumpIfTrue(8)
        // 4: PushVariable(2)
        // 5: PushScalar("value")
        // 6: Eq -> true
        // 7: Jump(9)
        // 8: PushScalar(true)
        let instructions = vec![
            OpCode::PushVariable(var_ref_principal(1)),
            OpCode::PushInteger(1),
            OpCode::Eq,              // -> false
            OpCode::JumpIfTrue(8),   // jump NOT taken
            OpCode::PushVariable(var_ref_principal(2)),
            OpCode::PushString("value".into()),
            OpCode::Eq,              // -> true
            OpCode::Jump(9),
            OpCode::PushBool(true),
        ];

        let result = vm.evaluate(&instructions);
        assert_eq!(result, ConditionResult::True);
    }
}
