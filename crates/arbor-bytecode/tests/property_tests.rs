use arbor_bytecode::compiler::BytecodeCompiler;
use arbor_bytecode::bytecode_vm::BytecodeVM;
use arbor_bytecode::evaluate_ast;
use arbor_types::{
    flatten_attributes, AttributeNameId, AttributeValue, Attributes, Condition, ConditionResult,
    EntityResolver, EntityTypeId, EvaluationContext, IndexedAttributeValue, IndexedEntity,
    Operand, SortedSetRef, VariableRef, VariableScope,
};
use proptest::prelude::*;
use std::collections::HashMap;
use chrono::{Utc, TimeZone};
use ordered_float::OrderedFloat;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

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
    /// Flattens `attrs` into this resolver's shared arena and attaches the
    /// resulting `SortedSetRef` to the already-inserted entity `idx`.
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
    fn attribute_pairs(&self, range: SortedSetRef) -> &[(AttributeNameId, IndexedAttributeValue)] {
        &self.pairs[range.offset as usize..(range.offset + range.len) as usize]
    }
    fn attribute_set_values(&self, range: SortedSetRef) -> &[IndexedAttributeValue] {
        &self.values[range.offset as usize..(range.offset + range.len) as usize]
    }
}

fn empty_entity(idx: u32) -> IndexedEntity {
    IndexedEntity {
        idx,
        attributes: SortedSetRef::EMPTY,
        entity_type: EntityTypeId::new(0),
        ancestors: SortedSetRef::EMPTY,
        principal_of_policies: None,
        resource_of_policies: None,
        effective_principal_policies: None,
        effective_resource_policies: None,
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

        let mut principal_attrs = Attributes::new();
        for (id, val) in p_attrs { principal_attrs.set(id, val); }
        let mut resource_attrs = Attributes::new();
        for (id, val) in r_attrs { resource_attrs.set(id, val); }
        let mut context_attrs = Attributes::new();
        for (id, val) in c_attrs { context_attrs.set(id, val); }

        let resolver = MapResolver::new()
            .insert(empty_entity(1), vec![])
            .insert(empty_entity(2), vec![])
            .with_attributes(1, &principal_attrs)
            .with_attributes(2, &resource_attrs);
        let principal = resolver.get_entity(1).unwrap().clone();
        let resource = resolver.get_entity(2).unwrap().clone();

        let context = EvaluationContext::new(&principal, &resource, Some(&context_attrs), &resolver);

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
