//! AST-based Condition Evaluator
//!
//! Ported from YSNP (ysnp-core/src/engine/condition_evaluator.rs)
//!
//! This provides an AST evaluation path that can be used as:
//! 1. Fallback if bytecode VM has issues
//! 2. Property testing (verify bytecode ≡ AST)
//! 3. Development/debugging tool
//!
//! The core evaluation logic is battle-tested from YSNP.

use arbor_types::{AttributeValue, Attributes, ScalarValue};
use arbor_types::{Condition, Operand, VariableRef, VariableScope};
use arbor_types::AttributeNameId;
use ordered_float::OrderedFloat;
use uuid::Uuid;

/// Result of evaluating a condition
#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationResult {
    True,
    False,
    Unknown(Vec<EvaluationNeed>), // Cannot be determined without more context
    Invalid(Vec<EvaluationError>), // Invalid operation or type error
}

/// Describes what information is needed to complete evaluation
#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationNeed {
    /// A variable (attribute path) could not be resolved
    MissingAttribute {
        scope: VariableScope,
        path: Vec<AttributeNameId>,
    },
}

/// Errors that can occur during condition evaluation
#[derive(Debug, Clone, PartialEq)]
pub enum EvaluationError {
    /// An entity reference is not found
    MissingEntity { entity_id: Uuid },

    /// Cannot compare these scalar types
    InvalidScalarComparison { l: ScalarValue, r: ScalarValue },

    /// Invalid types for a binary operation
    InvalidTypesForOperation {
        l: Option<Operand>,
        r: Option<Operand>,
    },

    /// Invalid type for a unary operation
    InvalidTypeForOperation { op: Operand },

    /// Feature not yet implemented
    UnimplementedFeature(String),
}

/// Trait for checking entity hierarchy relationships
///
/// This will be implemented by the Snapshot/Authorizer to provide
/// fast ancestor/descendant lookups using precomputed bitmaps.
pub trait GraphOracle {
    /// Check if `entity` is a descendant of `ancestor` in the hierarchy
    /// Returns Ok(true/false) if the relation is known, Err if entities don't exist
    fn is_descendant_of(&self, entity: Uuid, ancestor: Uuid) -> Result<bool, Vec<EvaluationError>>;
}

/// Context for evaluating conditions
///
/// Contains the principal, resource, and context attributes needed for variable resolution.
/// The graph_oracle provides entity hierarchy lookups for the `In` operator.
pub struct EvaluationContext<'a> {
    pub principal_attrs: &'a Attributes,
    pub resource_attrs: &'a Attributes,
    pub context_attrs: Option<&'a Attributes>,
    pub graph_oracle: &'a dyn GraphOracle,
}

/// AST-based condition evaluator (ported from YSNP)
pub struct AstEvaluator;

impl AstEvaluator {
    /// Evaluate a condition in the given context
    pub fn evaluate(condition: &Condition, context: &EvaluationContext<'_>) -> EvaluationResult {
        match condition {
            Condition::Operand(operand) => Self::evaluate_operand(operand, context),
            Condition::And(conds) => Self::evaluate_and(conds, context),
            Condition::Or(conds) => Self::evaluate_or(conds, context),
            Condition::Not(cond) => Self::evaluate_not(cond, context),

            Condition::Eq(l, r) => Self::evaluate_equality(l, r, context, true),
            Condition::Neq(l, r) => Self::evaluate_equality(l, r, context, false),

            Condition::Lt(l, r) => Self::evaluate_ordering(l, r, context, OrderOp::Lt),
            Condition::Lte(l, r) => Self::evaluate_ordering(l, r, context, OrderOp::Lte),
            Condition::Gt(l, r) => Self::evaluate_ordering(l, r, context, OrderOp::Gt),
            Condition::Gte(l, r) => Self::evaluate_ordering(l, r, context, OrderOp::Gte),

            Condition::In(l, r) => Self::evaluate_in(l, r, context),
            Condition::Contains(l, r) => Self::evaluate_contains(l, r, context),
            Condition::ContainsAll(l, r) => Self::evaluate_contains_all(l, r, context),
            Condition::ContainsAny(l, r) => Self::evaluate_contains_any(l, r, context),
            Condition::HasAttribute(op, attr) => Self::evaluate_has_attribute(op, *attr, context),
            Condition::InNetwork(_, _) => {
                EvaluationResult::Invalid(vec![EvaluationError::UnimplementedFeature(
                    "Condition::InNetwork()".to_string(),
                )])
            }
        }
    }

    fn evaluate_operand(operand: &Operand, context: &EvaluationContext<'_>) -> EvaluationResult {
        match Self::resolve_operand(operand, context) {
            Ok(Operand::Scalar(ScalarValue::Bool(b))) => b.into(),
            Ok(op) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypeForOperation { op }]),
            Err(needs) => EvaluationResult::Unknown(needs),
        }
    }

    fn evaluate_and(conds: &[Condition], context: &EvaluationContext<'_>) -> EvaluationResult {
        let mut has_unknown = false;
        let mut unknown_evaluation_needs = vec![];
        for cond in conds {
            match Self::evaluate(cond, context) {
                EvaluationResult::False => return EvaluationResult::False,
                EvaluationResult::Unknown(needs) => {
                    has_unknown = true;
                    unknown_evaluation_needs.extend(needs);
                }
                EvaluationResult::Invalid(errors) => return EvaluationResult::Invalid(errors),
                EvaluationResult::True => continue,
            }
        }
        if has_unknown {
            EvaluationResult::Unknown(unknown_evaluation_needs)
        } else {
            EvaluationResult::True
        }
    }

    fn evaluate_or(conds: &[Condition], context: &EvaluationContext<'_>) -> EvaluationResult {
        let mut has_unknown = false;
        let mut unknown_evaluation_needs = vec![];
        for cond in conds {
            match Self::evaluate(cond, context) {
                EvaluationResult::True => return EvaluationResult::True,
                EvaluationResult::Unknown(needs) => {
                    has_unknown = true;
                    unknown_evaluation_needs.extend(needs);
                }
                EvaluationResult::Invalid(errors) => return EvaluationResult::Invalid(errors),
                EvaluationResult::False => continue,
            }
        }
        if has_unknown {
            EvaluationResult::Unknown(unknown_evaluation_needs)
        } else {
            EvaluationResult::False
        }
    }

    fn evaluate_not(cond: &Condition, context: &EvaluationContext<'_>) -> EvaluationResult {
        match Self::evaluate(cond, context) {
            EvaluationResult::True => EvaluationResult::False,
            EvaluationResult::False => EvaluationResult::True,
            EvaluationResult::Unknown(needs) => EvaluationResult::Unknown(needs),
            EvaluationResult::Invalid(errors) => EvaluationResult::Invalid(errors),
        }
    }

    fn with_two<'a>(
        left: &'a Operand,
        right: &'a Operand,
        context: &EvaluationContext<'_>,
        function: impl FnOnce(Operand, Operand) -> EvaluationResult,
    ) -> EvaluationResult {
        let resolved_left = Self::resolve_operand(left, context);
        let resolved_right = Self::resolve_operand(right, context);
        match (resolved_left, resolved_right) {
            (Ok(left), Ok(right)) => function(left, right),
            (Err(mut left_needs), Err(right_needs)) => {
                left_needs.extend(right_needs);
                EvaluationResult::Unknown(left_needs)
            }
            (Err(needs), _) | (_, Err(needs)) => EvaluationResult::Unknown(needs),
        }
    }

    fn evaluate_equality(
        l: &Operand,
        r: &Operand,
        context: &EvaluationContext<'_>,
        expect_equal: bool,
    ) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| match (lv, rv) {
            (Operand::Scalar(ls), Operand::Scalar(rs)) => {
                (Self::scalar_eq(&ls, &rs) == expect_equal).into()
            }
            (lv, rv) => ((lv == rv) == expect_equal).into(),
        })
    }

    fn evaluate_ordering(
        l: &Operand,
        r: &Operand,
        context: &EvaluationContext<'_>,
        op: OrderOp,
    ) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| match (lv, rv) {
            (Operand::Scalar(ls), Operand::Scalar(rs)) => {
                match Self::compare_scalars(&ls, &rs, op) {
                    Ok(b) => b.into(),
                    Err(errors) => EvaluationResult::Invalid(errors),
                }
            }
            (Operand::Scalar(_), rv) => {
                EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: None,
                    r: Some(rv),
                }])
            }
            (lv, Operand::Scalar(_)) => {
                EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: Some(lv),
                    r: None,
                }])
            }
            (lv, rv) => {
                EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: Some(lv),
                    r: Some(rv),
                }])
            }
        })
    }

    fn evaluate_in(l: &Operand, r: &Operand, context: &EvaluationContext<'_>) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| match (lv, rv) {
            (Operand::EntityRef(le), Operand::EntityRef(re)) => {
                match context.graph_oracle.is_descendant_of(le, re) {
                    Ok(true) => EvaluationResult::True,
                    Ok(false) => EvaluationResult::False,
                    Err(errors) => EvaluationResult::Invalid(errors),
                }
            }
            (Operand::EntityRef(_), rv) => {
                EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: None,
                    r: Some(rv),
                }])
            }
            (lv, Operand::EntityRef(_)) => {
                EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: Some(lv),
                    r: None,
                }])
            }
            (lv, rv) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                l: Some(lv),
                r: Some(rv),
            }]),
        })
    }

    fn evaluate_contains(
        l: &Operand,
        r: &Operand,
        context: &EvaluationContext<'_>,
    ) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| {
            match (lv, rv) {
                (Operand::Set(set), val) => {
                    // Set should be fully resolved
                    set.contains(&val).into()
                }
                (lv, _) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: Some(lv),
                    r: None,
                }]),
            }
        })
    }

    fn evaluate_contains_all(
        l: &Operand,
        r: &Operand,
        context: &EvaluationContext<'_>,
    ) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| {
            match (lv, rv) {
                (Operand::Set(ls), Operand::Set(rs)) => {
                    // both sets should be fully resolved
                    ls.iter().all(|lv| rs.contains(lv)).into()
                }
                (Operand::Set(_), rv) => {
                    EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                        l: None,
                        r: Some(rv),
                    }])
                }
                (lv, Operand::Set(_)) => {
                    EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                        l: Some(lv),
                        r: None,
                    }])
                }
                (lv, rv) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                    l: Some(lv),
                    r: Some(rv),
                }]),
            }
        })
    }

    fn evaluate_contains_any(
        l: &Operand,
        r: &Operand,
        context: &EvaluationContext,
    ) -> EvaluationResult {
        Self::with_two(l, r, context, |lv, rv| match (lv, rv) {
            (Operand::Set(ls), Operand::Set(rs)) => ls.iter().any(|lv| rs.contains(lv)).into(),
            (Operand::Set(_), rv) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                l: None,
                r: Some(rv),
            }]),
            (lv, Operand::Set(_)) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                l: Some(lv),
                r: None,
            }]),
            (lv, rv) => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypesForOperation {
                l: Some(lv),
                r: Some(rv),
            }]),
        })
    }

    fn evaluate_has_attribute(
        op: &Operand,
        attr: AttributeNameId,
        context: &EvaluationContext,
    ) -> EvaluationResult {
        // Check attribute existence on principal/resource depending on variable reference
        match op {
            Operand::Variable(vr) => match vr.scope {
                VariableScope::Principal => context.principal_attrs.get(&attr).is_some().into(),
                VariableScope::Resource => context.resource_attrs.get(&attr).is_some().into(),
                VariableScope::Context => context
                    .context_attrs
                    .and_then(|ctx| ctx.get(&attr))
                    .is_some()
                    .into(),
            },
            operand => EvaluationResult::Invalid(vec![EvaluationError::InvalidTypeForOperation {
                op: operand.clone(),
            }]),
        }
    }

    fn resolve_operand(
        operand: &Operand,
        context: &EvaluationContext,
    ) -> Result<Operand, Vec<EvaluationNeed>> {
        match operand {
            Operand::Scalar(scalar) => Ok(Operand::Scalar(scalar.clone())),
            Operand::EntityRef(eid) => Ok(Operand::EntityRef(*eid)),
            Operand::Variable(var_ref) => {
                // Lookup variable in context
                let av_opt: Option<&AttributeValue> = match var_ref.scope {
                    VariableScope::Principal => context.principal_attrs.get_nested(&var_ref.path),
                    VariableScope::Resource => context.resource_attrs.get_nested(&var_ref.path),
                    VariableScope::Context => context
                        .context_attrs
                        .and_then(|ctx| ctx.get_nested(&var_ref.path)),
                };
                match av_opt.and_then(|av| Operand::try_from(av.clone()).ok()) {
                    Some(result) => Ok(result),
                    None => Err(vec![EvaluationNeed::MissingAttribute {
                        scope: var_ref.scope.clone(),
                        path: var_ref.path.clone(),
                    }]),
                }
            }
            Operand::Set(items) => {
                // Try to resolve each item; if any fails, keep original
                let mut out = Vec::with_capacity(items.len());
                let mut needs = Vec::new();
                for it in items {
                    match Self::resolve_operand(it, context) {
                        Ok(val) => out.push(val),
                        Err(e) => {
                            needs.extend(e);
                            out.push(it.clone());
                        }
                    }
                }
                if needs.is_empty() {
                    Ok(Operand::Set(out))
                } else {
                    Err(needs)
                }
            }
        }
    }
}

#[derive(Clone, Copy)]
enum OrderOp {
    Lt,
    Lte,
    Gt,
    Gte,
}

impl AstEvaluator {
    fn scalar_eq(a: &ScalarValue, b: &ScalarValue) -> bool {
        match (a, b) {
            (ScalarValue::Integer(ai), ScalarValue::Integer(bi)) => ai == bi,
            (ScalarValue::Float(af), ScalarValue::Float(bf)) => af == bf,
            (ScalarValue::Integer(ai), ScalarValue::Float(bf)) => OrderedFloat(*ai as f64) == *bf,
            (ScalarValue::Float(af), ScalarValue::Integer(bi)) => *af == (*bi as f64),
            (ScalarValue::String(as_), ScalarValue::String(bs_)) => as_ == bs_,
            (ScalarValue::Bool(ab), ScalarValue::Bool(bb)) => ab == bb,
            (ScalarValue::Timestamp(at), ScalarValue::Timestamp(bt)) => at == bt,
            _ => false,
        }
    }

    fn compare_scalars(
        a: &ScalarValue,
        b: &ScalarValue,
        op: OrderOp,
    ) -> Result<bool, Vec<EvaluationError>> {
        match (a, b) {
            (ScalarValue::Integer(ai), ScalarValue::Integer(bi)) => Ok(match op {
                OrderOp::Lt => ai < bi,
                OrderOp::Lte => ai <= bi,
                OrderOp::Gt => ai > bi,
                OrderOp::Gte => ai >= bi,
            }),
            (ScalarValue::Float(af), ScalarValue::Float(bf)) => Ok(match op {
                OrderOp::Lt => af < bf,
                OrderOp::Lte => af <= bf,
                OrderOp::Gt => af > bf,
                OrderOp::Gte => af >= bf,
            }),
            (ScalarValue::Integer(ai), ScalarValue::Float(bf)) => {
                let ai = OrderedFloat(*ai as f64);
                Ok(match op {
                    OrderOp::Lt => ai < *bf,
                    OrderOp::Lte => ai <= *bf,
                    OrderOp::Gt => ai > *bf,
                    OrderOp::Gte => ai >= *bf,
                })
            }
            (ScalarValue::Float(af), ScalarValue::Integer(bi)) => {
                let bi = OrderedFloat(*bi as f64);
                Ok(match op {
                    OrderOp::Lt => *af < bi,
                    OrderOp::Lte => *af <= bi,
                    OrderOp::Gt => *af > bi,
                    OrderOp::Gte => *af >= bi,
                })
            }
            (ScalarValue::String(as_), ScalarValue::String(bs_)) => Ok(match op {
                OrderOp::Lt => as_ < bs_,
                OrderOp::Lte => as_ <= bs_,
                OrderOp::Gt => as_ > bs_,
                OrderOp::Gte => as_ >= bs_,
            }),
            (ScalarValue::Timestamp(at), ScalarValue::Timestamp(bt)) => Ok(match op {
                OrderOp::Lt => at < bt,
                OrderOp::Lte => at <= bt,
                OrderOp::Gt => at > bt,
                OrderOp::Gte => at >= bt,
            }),
            _ => Err(vec![EvaluationError::InvalidScalarComparison {
                l: a.clone(),
                r: b.clone(),
            }]),
        }
    }
}

// Add this helper trait for converting bool to EvaluationResult
impl From<bool> for EvaluationResult {
    fn from(b: bool) -> Self {
        if b {
            EvaluationResult::True
        } else {
            EvaluationResult::False
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use arbor_types::{AttributeValue, Attributes, ScalarValue};
    use arbor_types::{Condition, Operand, VariableRef, VariableScope};
    use arbor_types::AttributeNameId;
    use uuid::Uuid;

    // Mock GraphOracle for testing
    struct MockGraphOracle {
        // Map of (entity, ancestor) -> is_descendant
        relationships: std::collections::HashMap<(Uuid, Uuid), bool>,
    }

    impl MockGraphOracle {
        fn new() -> Self {
            Self {
                relationships: std::collections::HashMap::new(),
            }
        }

        fn add_relationship(&mut self, entity: Uuid, ancestor: Uuid) {
            self.relationships.insert((entity, ancestor), true);
        }
    }

    impl GraphOracle for MockGraphOracle {
        fn is_descendant_of(&self, entity: Uuid, ancestor: Uuid) -> Result<bool, Vec<EvaluationError>> {
            Ok(*self.relationships.get(&(entity, ancestor)).unwrap_or(&false))
        }
    }

    #[test]
    fn test_simple_boolean() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: true operand
        let cond = Condition::Operand(Operand::Scalar(ScalarValue::Bool(true)));
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);

        // Test: false operand
        let cond = Condition::Operand(Operand::Scalar(ScalarValue::Bool(false)));
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::False);
    }

    #[test]
    fn test_and_logic() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: true AND true = true
        let cond = Condition::And(vec![
            Condition::Operand(Operand::Scalar(ScalarValue::Bool(true))),
            Condition::Operand(Operand::Scalar(ScalarValue::Bool(true))),
        ]);
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);

        // Test: true AND false = false
        let cond = Condition::And(vec![
            Condition::Operand(Operand::Scalar(ScalarValue::Bool(true))),
            Condition::Operand(Operand::Scalar(ScalarValue::Bool(false))),
        ]);
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::False);
    }

    #[test]
    fn test_equality() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: 42 == 42
        let cond = Condition::Eq(
            Operand::Scalar(ScalarValue::Integer(42)),
            Operand::Scalar(ScalarValue::Integer(42)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);

        // Test: 42 == 43
        let cond = Condition::Eq(
            Operand::Scalar(ScalarValue::Integer(42)),
            Operand::Scalar(ScalarValue::Integer(43)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::False);

        // Test: "hello" == "hello"
        let cond = Condition::Eq(
            Operand::Scalar(ScalarValue::String("hello".to_string())),
            Operand::Scalar(ScalarValue::String("hello".to_string())),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);
    }

    #[test]
    fn test_ordering() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: 10 < 20
        let cond = Condition::Lt(
            Operand::Scalar(ScalarValue::Integer(10)),
            Operand::Scalar(ScalarValue::Integer(20)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);

        // Test: 20 < 10
        let cond = Condition::Lt(
            Operand::Scalar(ScalarValue::Integer(20)),
            Operand::Scalar(ScalarValue::Integer(10)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::False);
    }

    #[test]
    fn test_variable_resolution() {
        let oracle = MockGraphOracle::new();
        let mut principal_attrs = Attributes::new();
        let level_attr = AttributeNameId::new(1);
        principal_attrs.set(level_attr, AttributeValue::Scalar(ScalarValue::Integer(5)));

        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: principal.level == 5
        let var_ref = VariableRef {
            scope: VariableScope::Principal,
            path: vec![level_attr],
        };
        let cond = Condition::Eq(
            Operand::Variable(var_ref),
            Operand::Scalar(ScalarValue::Integer(5)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);
    }

    #[test]
    fn test_missing_attribute() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: principal.missing_attr == 5 should return Unknown
        let missing_attr = AttributeNameId::new(999);
        let var_ref = VariableRef {
            scope: VariableScope::Principal,
            path: vec![missing_attr],
        };
        let cond = Condition::Eq(
            Operand::Variable(var_ref.clone()),
            Operand::Scalar(ScalarValue::Integer(5)),
        );
        let result = AstEvaluator::evaluate(&cond, &ctx);
        match result {
            EvaluationResult::Unknown(needs) => {
                assert_eq!(needs.len(), 1);
                assert_eq!(
                    needs[0],
                    EvaluationNeed::MissingAttribute {
                        scope: VariableScope::Principal,
                        path: vec![missing_attr]
                    }
                );
            }
            _ => panic!("Expected Unknown result"),
        }
    }

    #[test]
    fn test_contains() {
        let oracle = MockGraphOracle::new();
        let principal_attrs = Attributes::new();
        let resource_attrs = Attributes::new();
        let ctx = EvaluationContext {
            principal_attrs: &principal_attrs,
            resource_attrs: &resource_attrs,
            context_attrs: None,
            graph_oracle: &oracle,
        };

        // Test: [1, 2, 3] contains 2
        let set = Operand::Set(vec![
            Operand::Scalar(ScalarValue::Integer(1)),
            Operand::Scalar(ScalarValue::Integer(2)),
            Operand::Scalar(ScalarValue::Integer(3)),
        ]);
        let val = Operand::Scalar(ScalarValue::Integer(2));
        let cond = Condition::Contains(set.clone(), val);
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::True);

        // Test: [1, 2, 3] contains 5
        let val = Operand::Scalar(ScalarValue::Integer(5));
        let cond = Condition::Contains(set, val);
        let result = AstEvaluator::evaluate(&cond, &ctx);
        assert_eq!(result, EvaluationResult::False);
    }
}
