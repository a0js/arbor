#![cfg(any(test, feature = "test_utils"))]
use arbor_types::{
    resolve_nested_attribute, Attributes, AttributeValue, Condition, ConditionResult,
    EntityResolver, EvaluationContext, EvaluationError, IndexedAttributeValue, Operand,
    VariableRef, VariableScope,
};
use chrono::{DateTime, Utc};
use ipnet::IpNet;
use ordered_float::OrderedFloat;
use std::net::IpAddr;
use std::cmp::Ordering;

#[derive(Debug, Clone, PartialEq)]
enum Val {
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

impl From<AttributeValue> for Val {
    fn from(av: AttributeValue) -> Self {
        match av {
            AttributeValue::Integer(i) => Val::Integer(i),
            AttributeValue::Float(f) => Val::Float(f),
            AttributeValue::Timestamp(t) => Val::Timestamp(t),
            AttributeValue::String(s) => Val::String(s),
            AttributeValue::Bool(b) => Val::Bool(b),
            AttributeValue::EntityRef(u) => Val::EntityRef(u),
            AttributeValue::IpAddr(ip) => Val::IpAddr(ip),
            AttributeValue::IpNetwork(net) => Val::IpNetwork(net),
            AttributeValue::Set(s) => Val::Set(s),
            AttributeValue::Object(_) => Val::Missing,
        }
    }
}

/// Evaluates a condition AST directly, providing a reference implementation
/// to compare against the bytecode VM.
pub fn evaluate_ast(condition: &Condition, context: &EvaluationContext) -> ConditionResult {
    match eval_cond(condition, context) {
        Ok(true) => ConditionResult::True,
        Ok(false) => ConditionResult::False,
        Err(e) => ConditionResult::Invalid(vec![e]),
    }
}

fn eval_cond(condition: &Condition, context: &EvaluationContext) -> Result<bool, EvaluationError> {
    match condition {
        Condition::Operand(op) => {
            match eval_operand(op, context)? {
                Val::Bool(b) => Ok(b),
                Val::Missing => Err(EvaluationError::ExecutionError("compiler bug: Missing reached JumpIfFalse".into())),
                other => Err(EvaluationError::ExecutionError(format!("JumpIfFalse requires Bool, got {:?}", other))),
            }
        }
        Condition::And(conds) => {
            if conds.is_empty() { return Ok(true); }
            for c in conds {
                if !eval_cond(c, context)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        Condition::Or(conds) => {
            if conds.is_empty() { return Ok(false); }
            for c in conds {
                if eval_cond(c, context)? {
                    return Ok(true);
                }
            }
            Ok(false)
        }
        Condition::Not(inner) => {
            let res = eval_cond(inner, context)?;
            Ok(!res)
        }

        Condition::Eq(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(val_scalar_eq(&lv, &rv))
                }
                (Val::EntityRef(l), Val::EntityRef(r)) => Ok(l == r),
                (Val::Bool(l), Val::Bool(r)) => Ok(l == r),
                (Val::IpAddr(l), Val::IpAddr(r)) => Ok(l == r),
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in ==: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::Neq(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(!val_scalar_eq(&lv, &rv))
                }
                (Val::EntityRef(l), Val::EntityRef(r)) => Ok(l != r),
                (Val::Bool(l), Val::Bool(r)) => Ok(l != r),
                (Val::IpAddr(l), Val::IpAddr(r)) => Ok(l != r),
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in !=: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::Lt(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(val_scalar_cmp(&lv, &rv)? == Ordering::Less)
                }
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in <: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::Lte(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(val_scalar_cmp(&lv, &rv)? != Ordering::Greater)
                }
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in <=: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::Gt(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(val_scalar_cmp(&lv, &rv)? == Ordering::Greater)
                }
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in >: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::Gte(l, r) => {
            let lv = eval_operand(l, context)?;
            let rv = eval_operand(r, context)?;
            match (&lv, &rv) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::Integer(_), Val::Integer(_))
                | (Val::Integer(_), Val::Float(_))
                | (Val::Float(_), Val::Integer(_))
                | (Val::Float(_), Val::Float(_))
                | (Val::String(_), Val::String(_))
                | (Val::Timestamp(_), Val::Timestamp(_)) => {
                    Ok(val_scalar_cmp(&lv, &rv)? != Ordering::Less)
                }
                _ => Err(EvaluationError::ExecutionError(format!("type mismatch in >=: {:?} vs {:?}", lv, rv))),
            }
        }
        Condition::In(elem, set) => {
            match (eval_operand(elem, context)?, eval_operand(set, context)?) {
                (ev, Val::Set(items)) => {
                    if let Val::Missing = ev { return Ok(false); }
                    for i in items {
                        if val_attr_eq(&ev, &i) {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
                (_, Val::Missing) => Ok(false),
                (_, sv) => Err(EvaluationError::ExecutionError(format!("In requires a set, got {:?}", sv))),
            }
        }
        Condition::Contains(set, elem) => {
            match (eval_operand(set, context)?, eval_operand(elem, context)?) {
                (Val::Set(items), ev) => {
                    if let Val::Missing = ev { return Ok(false); }
                    for i in items {
                        if val_attr_eq(&ev, &i) {
                            return Ok(true);
                        }
                    }
                    Ok(false)
                }
                (Val::Missing, _) => Ok(false),
                (sv, _) => Err(EvaluationError::ExecutionError(format!("Contains requires a set, got {:?}", sv))),
            }
        }
        Condition::ContainsAll(set, subset) => {
            let sv = eval_operand(set, context)?;
            let ssv = eval_operand(subset, context)?;
            match (sv, ssv) {
                (Val::Set(items), Val::Set(sub_items)) => {
                    for si in sub_items {
                        let mut found = false;
                        for i in &items {
                            if attr_val_eq(&si, i) {
                                found = true;
                                break;
                            }
                        }
                        if !found { return Ok(false); }
                    }
                    Ok(true)
                }
                (Val::Missing, Val::Set(_)) => Ok(false),
                (Val::Set(_), Val::Missing) => Ok(false),
                (Val::Missing, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("ContainsAll requires two sets".into())),
            }
        }
        Condition::ContainsAny(set, subset) => {
            let sv = eval_operand(set, context)?;
            let ssv = eval_operand(subset, context)?;
            match (sv, ssv) {
                (Val::Set(items), Val::Set(sub_items)) => {
                    for si in sub_items {
                        for i in &items {
                            if attr_val_eq(&si, i) {
                                return Ok(true);
                            }
                        }
                    }
                    Ok(false)
                }
                (Val::Missing, Val::Set(_)) => Ok(false),
                (Val::Set(_), Val::Missing) => Ok(false),
                (Val::Missing, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("ContainsAny requires two sets".into())),
            }
        }

        Condition::StartsWith(s, prefix) => {
            let sv = eval_operand(s, context)?;
            let pv = eval_operand(prefix, context)?;
            match (sv, pv) {
                (Val::String(s), Val::String(p)) => Ok(s.starts_with(&p)),
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("StartsWith requires strings".into())),
            }
        }
        Condition::EndsWith(s, suffix) => {
            let sv = eval_operand(s, context)?;
            let pv = eval_operand(suffix, context)?;
            match (sv, pv) {
                (Val::String(s), Val::String(p)) => Ok(s.ends_with(&p)),
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("EndsWith requires strings".into())),
            }
        }
        Condition::StringContains(haystack, needle) => {
            let hv = eval_operand(haystack, context)?;
            let nv = eval_operand(needle, context)?;
            match (hv, nv) {
                (Val::String(h), Val::String(n)) => Ok(h.contains(&n)),
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("StringContains requires strings".into())),
            }
        }
        Condition::Like(s, pattern) => {
            let sv = eval_operand(s, context)?;
            let pv = eval_operand(pattern, context)?;
            match (sv, pv) {
                (Val::String(s), Val::String(p)) => Ok(glob_match(&s, &p)),
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                _ => Err(EvaluationError::ExecutionError("Like requires strings".into())),
            }
        }

        Condition::HasAttribute(op, attr_id) => {
            let v_ref = match op {
                Operand::Variable(v) => v,
                _ => return Err(EvaluationError::ExecutionError("HasAttribute requires a Variable".into())),
            };
            let mut full_path = v_ref.path.clone();
            full_path.push(*attr_id);
            let check_ref = VariableRef { scope: v_ref.scope.clone(), path: full_path };
            Ok(resolve_variable(&check_ref, context).is_some())
        }

        Condition::IsType(scope, type_id) => {
            let entity_type = match scope {
                VariableScope::Principal => context.principal.entity_type,
                VariableScope::Resource => context.resource.entity_type,
                VariableScope::Context => return Err(EvaluationError::ExecutionError("IsType on Context".into())),
            };
            Ok(entity_type == *type_id)
        }

        Condition::InHierarchy(left, right) => {
            let lv = eval_operand(left, context)?;
            let rv = eval_operand(right, context)?;
            let target_idx = match rv {
                Val::EntityRef(idx) => idx,
                Val::Missing => return Ok(false),
                _ => return Err(EvaluationError::ExecutionError("InHierarchy requires EntityRef right".into())),
            };
            let entity_idx = match lv {
                Val::EntityRef(idx) => idx,
                Val::Missing => return Ok(false),
                _ => return Err(EvaluationError::ExecutionError("InHierarchy requires EntityRef left".into())),
            };
            match context.entities.ancestors_of(entity_idx) {
                Some(ancestors) => Ok(ancestors.binary_search(&target_idx).is_ok()),
                None => Err(EvaluationError::MissingEntity { entity_index: entity_idx }),
            }
        }

        Condition::InNetwork(ip_op, net_op) => {
            match (eval_operand(ip_op, context)?, eval_operand(net_op, context)?) {
                (Val::Missing, _) | (_, Val::Missing) => Ok(false),
                (Val::IpAddr(ip), Val::IpNetwork(net)) => Ok(net.contains(&ip)),
                _ => Err(EvaluationError::ExecutionError(
                    "InNetwork: expected (IpAddr, IpNetwork)".into(),
                )),
            }
        }
    }
}

fn eval_operand(op: &Operand, context: &EvaluationContext) -> Result<Val, EvaluationError> {
    match op {
        Operand::String(s) => Ok(Val::String(s.clone())),
        Operand::Integer(i) => Ok(Val::Integer(*i)),
        Operand::Float(f) => Ok(Val::Float(*f)),
        Operand::Bool(b) => Ok(Val::Bool(*b)),
        Operand::Timestamp(t) => Ok(Val::Timestamp(*t)),
        Operand::IpAddr(ip) => Ok(Val::IpAddr(*ip)),
        Operand::IpNetwork(net) => Ok(Val::IpNetwork(*net)),
        Operand::EntityRef(u) => Ok(Val::EntityRef(*u)),
        Operand::Set(items) => {
            let mut vals = Vec::new();
            for item in items {
                match eval_operand(item, context)? {
                    Val::Missing => return Err(EvaluationError::ExecutionError("compiler bug: Missing in Set".into())),
                    Val::Integer(i) => vals.push(AttributeValue::Integer(i)),
                    Val::Float(f) => vals.push(AttributeValue::Float(f)),
                    Val::Timestamp(t) => vals.push(AttributeValue::Timestamp(t)),
                    Val::String(s) => vals.push(AttributeValue::String(s)),
                    Val::Bool(b) => vals.push(AttributeValue::Bool(b)),
                    Val::EntityRef(u) => vals.push(AttributeValue::EntityRef(u)),
                    Val::IpAddr(ip) => vals.push(AttributeValue::IpAddr(ip)),
                    Val::IpNetwork(net) => vals.push(AttributeValue::IpNetwork(net)),
                    Val::Set(s) => vals.push(AttributeValue::Set(s)),
                }
            }
            Ok(Val::Set(vals))
        }
        Operand::Variable(v) => Ok(resolve_variable(v, context).unwrap_or(Val::Missing)),
    }
}

/// Principal/Resource attributes resolve through `context.entities`' shared
/// attribute arena (`IndexedAttributeValue`); Context attributes are a
/// plain, per-request `Attributes` (never persisted, never arena-backed) --
/// see `bytecode_vm::resolve_variable` for the identical split, kept
/// independently implemented here since this file exists specifically to
/// cross-check the VM's semantics.
fn resolve_variable(var_ref: &VariableRef, context: &EvaluationContext) -> Option<Val> {
    match var_ref.scope {
        VariableScope::Context => {
            let value = context.context_attrs?.get_nested(&var_ref.path)?;
            Some(Val::from(value.clone()))
        }
        VariableScope::Principal | VariableScope::Resource => {
            let base = match var_ref.scope {
                VariableScope::Principal => context.principal.attributes,
                VariableScope::Resource => context.resource.attributes,
                VariableScope::Context => unreachable!(),
            };
            let value = resolve_nested_attribute(context.entities, base, &var_ref.path)?;
            Some(indexed_attribute_value_to_val(context.entities, value))
        }
    }
}

fn indexed_attribute_value_to_val(entities: &dyn EntityResolver, v: &IndexedAttributeValue) -> Val {
    match v {
        IndexedAttributeValue::String(s) => Val::String(s.clone()),
        IndexedAttributeValue::Float(f) => Val::Float(*f),
        IndexedAttributeValue::Integer(i) => Val::Integer(*i),
        IndexedAttributeValue::Bool(b) => Val::Bool(*b),
        IndexedAttributeValue::IpAddr(ip) => Val::IpAddr(*ip),
        IndexedAttributeValue::IpNetwork(net) => Val::IpNetwork(*net),
        IndexedAttributeValue::Timestamp(t) => Val::Timestamp(*t),
        IndexedAttributeValue::EntityRef(u) => Val::EntityRef(*u),
        IndexedAttributeValue::Set(set_ref) => Val::Set(
            entities
                .attribute_set_values(*set_ref)
                .iter()
                .map(|e| indexed_to_attribute_value(entities, e))
                .collect(),
        ),
        IndexedAttributeValue::Object(_) => Val::Missing,
    }
}

/// Converts an arena-backed `IndexedAttributeValue` into an owned
/// `AttributeValue`, for materializing `Set` elements -- mirrors
/// `bytecode_vm`'s helper of the same shape.
fn indexed_to_attribute_value(entities: &dyn EntityResolver, v: &IndexedAttributeValue) -> AttributeValue {
    match v {
        IndexedAttributeValue::String(s) => AttributeValue::String(s.clone()),
        IndexedAttributeValue::Float(f) => AttributeValue::Float(*f),
        IndexedAttributeValue::Integer(i) => AttributeValue::Integer(*i),
        IndexedAttributeValue::Bool(b) => AttributeValue::Bool(*b),
        IndexedAttributeValue::IpAddr(ip) => AttributeValue::IpAddr(*ip),
        IndexedAttributeValue::IpNetwork(net) => AttributeValue::IpNetwork(net.clone()),
        IndexedAttributeValue::Timestamp(t) => AttributeValue::Timestamp(*t),
        IndexedAttributeValue::EntityRef(u) => AttributeValue::EntityRef(*u),
        IndexedAttributeValue::Set(set_ref) => AttributeValue::Set(
            entities
                .attribute_set_values(*set_ref)
                .iter()
                .map(|e| indexed_to_attribute_value(entities, e))
                .collect(),
        ),
        IndexedAttributeValue::Object(obj_ref) => {
            let mut attrs = Attributes::new();
            for (name, value) in entities.attribute_pairs(*obj_ref) {
                attrs.set(*name, indexed_to_attribute_value(entities, value));
            }
            AttributeValue::Object(attrs)
        }
    }
}

fn val_scalar_eq(a: &Val, b: &Val) -> bool {
    match (a, b) {
        (Val::Integer(ai), Val::Integer(bi)) => ai == bi,
        (Val::Float(af), Val::Float(bf)) => af == bf,
        (Val::Integer(ai), Val::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
        (Val::Float(af), Val::Integer(bi)) => *af == OrderedFloat(*bi as f64),
        (Val::String(a), Val::String(b)) => a == b,
        (Val::Timestamp(a), Val::Timestamp(b)) => a == b,
        _ => false,
    }
}

fn val_scalar_cmp(a: &Val, b: &Val) -> Result<Ordering, EvaluationError> {
    match (a, b) {
        (Val::Integer(ai), Val::Integer(bi)) => Ok(ai.cmp(bi)),
        (Val::Float(af), Val::Float(bf)) => Ok(af.cmp(bf)),
        (Val::Integer(ai), Val::Float(bf)) => Ok(OrderedFloat(*ai as f64).cmp(bf)),
        (Val::Float(af), Val::Integer(bi)) => Ok(af.cmp(&OrderedFloat(*bi as f64))),
        (Val::String(a), Val::String(b)) => Ok(a.cmp(b)),
        (Val::Timestamp(a), Val::Timestamp(b)) => Ok(a.cmp(b)),
        _ => unreachable!(),
    }
}

fn val_attr_eq(v: &Val, av: &AttributeValue) -> bool {
    match (v, av) {
        (Val::Integer(ai), AttributeValue::Integer(bi)) => ai == bi,
        (Val::Float(af), AttributeValue::Float(bf)) => af == bf,
        (Val::Integer(ai), AttributeValue::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
        (Val::Float(af), AttributeValue::Integer(bi)) => *af == OrderedFloat(*bi as f64),
        (Val::String(a), AttributeValue::String(b)) => a == b,
        (Val::Bool(a), AttributeValue::Bool(b)) => a == b,
        (Val::Timestamp(a), AttributeValue::Timestamp(b)) => a == b,
        (Val::IpAddr(a), AttributeValue::IpAddr(b)) => a == b,
        (Val::EntityRef(a), AttributeValue::EntityRef(b)) => a == b,
        _ => false,
    }
}

fn attr_val_eq(av: &AttributeValue, v: &AttributeValue) -> bool {
    match (av, v) {
        (AttributeValue::Integer(ai), AttributeValue::Integer(bi)) => ai == bi,
        (AttributeValue::Float(af), AttributeValue::Float(bf)) => af == bf,
        (AttributeValue::Integer(ai), AttributeValue::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
        (AttributeValue::Float(af), AttributeValue::Integer(bi)) => *af == OrderedFloat(*bi as f64),
        (AttributeValue::String(a), AttributeValue::String(b)) => a == b,
        (AttributeValue::Bool(a), AttributeValue::Bool(b)) => a == b,
        (AttributeValue::Timestamp(a), AttributeValue::Timestamp(b)) => a == b,
        (AttributeValue::IpAddr(a), AttributeValue::IpAddr(b)) => a == b,
        (AttributeValue::EntityRef(a), AttributeValue::EntityRef(b)) => a == b,
        _ => false,
    }
}

fn glob_match(s: &str, pattern: &str) -> bool {
    let mut s_idx = 0;
    let mut p_idx = 0;
    let mut star_idx = -1;
    let mut s_tmp_idx = -1;

    let s_chars: Vec<char> = s.chars().collect();
    let p_chars: Vec<char> = pattern.chars().collect();

    while s_idx < s_chars.len() {
        if p_idx < p_chars.len() && p_chars[p_idx] == '*' {
            star_idx = p_idx as i32;
            s_tmp_idx = s_idx as i32;
            p_idx += 1;
        } else if p_idx < p_chars.len() && (p_chars[p_idx] == s_chars[s_idx]) {
            s_idx += 1;
            p_idx += 1;
        } else if star_idx != -1 {
            p_idx = (star_idx + 1) as usize;
            s_tmp_idx += 1;
            s_idx = s_tmp_idx as usize;
        } else {
            return false;
        }
    }

    while p_idx < p_chars.len() && p_chars[p_idx] == '*' {
        p_idx += 1;
    }

    p_idx == p_chars.len()
}
