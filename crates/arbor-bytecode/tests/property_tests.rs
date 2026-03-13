use arbor_bytecode::compiler::BytecodeCompiler;
use arbor_bytecode::bytecode_vm::BytecodeVM;
use arbor_bytecode::evaluate_ast;
use arbor_types::{
    AttributeNameId, AttributeValue, Attributes, Condition, ConditionResult,
    EntityResolver, EntityTypeId, EvaluationContext, IndexedEntity, Operand, VariableRef, VariableScope,
};
use proptest::prelude::*;
use roaring::RoaringBitmap;
use chrono::{Utc, TimeZone};
use ordered_float::OrderedFloat;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

struct NoopResolver;
impl EntityResolver for NoopResolver {
    fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
}

fn empty_entity(idx: u32) -> IndexedEntity {
    IndexedEntity {
        idx,
        attributes: Attributes::new(),
        entity_type: EntityTypeId::new(0),
        descendants: RoaringBitmap::new(),
        ancestors: RoaringBitmap::new(),
        principal_of_policies: None,
        resource_of_policies: None,
    }
}

// ---------------------------------------------------------------------------
// Strategies
// ---------------------------------------------------------------------------

fn arb_attribute_value() -> BoxedStrategy<AttributeValue> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(AttributeValue::Integer),
        any::<f64>().prop_map(|f| AttributeValue::Float(OrderedFloat(f))),
        any::<bool>().prop_map(AttributeValue::Bool),
        ".*".prop_map(AttributeValue::String),
        any::<u32>().prop_map(AttributeValue::EntityRef),
    ];
    leaf.prop_recursive(3, 16, 8, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(AttributeValue::Set)
    }).boxed()
}

fn arb_variable_scope() -> impl Strategy<Value = VariableScope> {
    prop_oneof![
        Just(VariableScope::Principal),
        Just(VariableScope::Resource),
        Just(VariableScope::Context),
    ]
}

fn arb_variable_ref() -> impl Strategy<Value = VariableRef> {
    (arb_variable_scope(), prop::collection::vec(any::<u32>().prop_map(AttributeNameId::new), 1..2))
        .prop_map(|(scope, path)| VariableRef { scope, path })
}

fn arb_operand() -> BoxedStrategy<Operand> {
    let leaf = prop_oneof![
        any::<i64>().prop_map(Operand::Integer),
        any::<f64>().prop_map(|f| Operand::Float(OrderedFloat(f))),
        any::<bool>().prop_map(Operand::Bool),
        ".*".prop_map(Operand::String),
        any::<u32>().prop_map(Operand::EntityRef),
        arb_variable_ref().prop_map(Operand::Variable),
    ];
    leaf.prop_recursive(3, 16, 8, |inner| {
        prop::collection::vec(inner, 0..4).prop_map(Operand::Set)
    }).boxed()
}

fn arb_condition() -> BoxedStrategy<Condition> {
    let leaf = prop_oneof![
        any::<bool>().prop_map(|b| Condition::Operand(Operand::Bool(b))),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Eq(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Neq(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Lt(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Lte(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Gt(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Gte(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::In(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Contains(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::ContainsAll(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::ContainsAny(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::StartsWith(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::EndsWith(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::StringContains(l, r)),
        (arb_operand(), arb_operand()).prop_map(|(l, r)| Condition::Like(l, r)),
        (arb_operand(), any::<u32>().prop_map(AttributeNameId::new))
            .prop_map(|(op, id)| Condition::HasAttribute(op, id)),
        (arb_variable_scope(), any::<u32>().prop_map(EntityTypeId::new))
            .prop_map(|(s, id)| Condition::IsType(s, id)),
    ];

    leaf.prop_recursive(4, 32, 8, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..4).prop_map(Condition::And),
            prop::collection::vec(inner.clone(), 0..4).prop_map(Condition::Or),
            inner.prop_map(|c| Condition::Not(Box::new(c))),
        ]
    }).boxed()
}

// ---------------------------------------------------------------------------
// Property: bytecode VM and AST evaluator always agree
// ---------------------------------------------------------------------------

proptest! {
    #![proptest_config(ProptestConfig::with_cases(500))]
    #[test]
    fn test_bytecode_vs_ast_equivalence(
        condition in arb_condition(),
        p_attrs in prop::collection::hash_map(
            any::<u32>().prop_map(AttributeNameId::new), arb_attribute_value(), 0..10),
        r_attrs in prop::collection::hash_map(
            any::<u32>().prop_map(AttributeNameId::new), arb_attribute_value(), 0..10),
        c_attrs in prop::collection::hash_map(
            any::<u32>().prop_map(AttributeNameId::new), arb_attribute_value(), 0..10),
    ) {
        let compiled = match BytecodeCompiler::new().compile(&condition) {
            Ok(c) => c,
            Err(_) => return Ok(()), // Skip structurally invalid conditions
        };

        let mut principal = empty_entity(1);
        for (id, val) in p_attrs { principal.attributes.set(id, val); }
        let mut resource = empty_entity(2);
        for (id, val) in r_attrs { resource.attributes.set(id, val); }
        let mut context_attrs = Attributes::new();
        for (id, val) in c_attrs { context_attrs.set(id, val); }

        let context = EvaluationContext::new(&principal, &resource, Some(&context_attrs), &NoopResolver);

        let ast_result = evaluate_ast(&condition, &context);
        let bc_result  = BytecodeVM::new().evaluate(&compiled.instructions, &context);

        match (&ast_result, &bc_result) {
            (ConditionResult::True,    ConditionResult::True)    => {}
            (ConditionResult::False,   ConditionResult::False)   => {}
            (ConditionResult::Invalid(_), ConditionResult::Invalid(_)) => {}
            _ => panic!(
                "Equivalence failed!\ncondition: {:?}\nAST: {:?}\nVM:  {:?}",
                condition, ast_result, bc_result
            ),
        }
    }
}
