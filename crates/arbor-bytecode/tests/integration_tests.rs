use arbor_bytecode::compiler::BytecodeCompiler;
use arbor_bytecode::bytecode_vm::BytecodeVM;
use arbor_types::{
    AttributeNameId, AttributeValue, Attributes, Condition, ConditionResult,
    EntityResolver, EntityTypeId, EvaluationContext, IndexedEntity, Operand, VariableRef, VariableScope,
};
use roaring::RoaringBitmap;
use chrono::{Utc, TimeZone};
use std::net::IpAddr;
use std::collections::HashMap;
use ordered_float::OrderedFloat;

// ---------------------------------------------------------------------------
// Test infrastructure
// ---------------------------------------------------------------------------

struct NoopResolver;
impl EntityResolver for NoopResolver {
    fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
}

struct MapResolver(HashMap<u32, IndexedEntity>);
impl MapResolver {
    fn new() -> Self { Self(HashMap::new()) }
    fn insert(mut self, entity: IndexedEntity) -> Self {
        self.0.insert(entity.idx, entity);
        self
    }
}
impl EntityResolver for MapResolver {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        self.0.get(&index)
    }
}

fn empty_entity_at(idx: u32) -> IndexedEntity {
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
// Tests
// ---------------------------------------------------------------------------

#[test]
fn test_all_operand_types_integration() {
    let float_attr     = AttributeNameId::new(1);
    let timestamp_attr = AttributeNameId::new(2);
    let ip_attr        = AttributeNameId::new(3);

    let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let ip: IpAddr = "192.168.1.1".parse().unwrap();

    let mut principal = empty_entity_at(1);
    principal.attributes.set(float_attr,     AttributeValue::Float(OrderedFloat(1.5)));
    principal.attributes.set(timestamp_attr, AttributeValue::Timestamp(now));
    principal.attributes.set(ip_attr,        AttributeValue::IpAddr(ip));

    let condition = Condition::And(vec![
        Condition::Gt(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![float_attr] }),
            Operand::Float(OrderedFloat(1.0)),
        ),
        Condition::Eq(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![timestamp_attr] }),
            Operand::Timestamp(now),
        ),
        Condition::Eq(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![ip_attr] }),
            Operand::IpAddr(ip),
        ),
    ]);

    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let resource = empty_entity_at(2);
    let context = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    let mut vm = BytecodeVM::new();

    assert_eq!(vm.evaluate(&compiled.instructions, &context), ConditionResult::True);
}

#[test]
fn test_complex_nested_logic_integration() {
    let age_attr    = AttributeNameId::new(1);
    let locked_attr = AttributeNameId::new(2);
    let public_attr = AttributeNameId::new(3);
    let role_attr   = AttributeNameId::new(4);

    let condition = Condition::Not(Box::new(Condition::And(vec![
        Condition::Or(vec![
            Condition::Lt(
                Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![age_attr] }),
                Operand::Integer(18),
            ),
            Condition::Eq(
                Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![locked_attr] }),
                Operand::Bool(true),
            ),
        ]),
        Condition::Or(vec![
            Condition::Eq(
                Operand::Variable(VariableRef { scope: VariableScope::Resource, path: vec![public_attr] }),
                Operand::Bool(true),
            ),
            Condition::Eq(
                Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![role_attr] }),
                Operand::String("admin".to_string()),
            ),
        ]),
    ])));

    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let mut resource = empty_entity_at(2);
    resource.attributes.set(public_attr, AttributeValue::Bool(false));

    // Not(young_or_locked AND public_or_admin) — age=25, !locked, role=user, !public → True
    let mut principal = empty_entity_at(1);
    principal.attributes.set(age_attr,    AttributeValue::Integer(25));
    principal.attributes.set(locked_attr, AttributeValue::Bool(false));
    principal.attributes.set(role_attr,   AttributeValue::String("user".to_string()));
    let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx), ConditionResult::True);

    // locked=true, role=user, !public → still True (left And branch fails but right returns user)
    principal.attributes.set(locked_attr, AttributeValue::Bool(true));
    let ctx2 = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx2), ConditionResult::True);

    // locked=true, role=admin, !public → False
    principal.attributes.set(role_attr, AttributeValue::String("admin".to_string()));
    let ctx3 = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx3), ConditionResult::False);
}

#[test]
fn test_all_operators_integration() {
    let s_attr   = AttributeNameId::new(1);
    let n_attr   = AttributeNameId::new(2);
    let set_attr = AttributeNameId::new(3);

    let condition = Condition::And(vec![
        Condition::StartsWith(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }),
            Operand::String("pre".to_string()),
        ),
        Condition::EndsWith(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }),
            Operand::String("fix".to_string()),
        ),
        Condition::StringContains(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }),
            Operand::String("mid".to_string()),
        ),
        Condition::Like(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }),
            Operand::String("pre*fix".to_string()),
        ),
        Condition::Neq(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![n_attr] }),
            Operand::Integer(0),
        ),
        Condition::Lte(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![n_attr] }),
            Operand::Integer(100),
        ),
        Condition::In(
            Operand::Integer(50),
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
        ),
        Condition::Contains(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
            Operand::Integer(50),
        ),
        Condition::ContainsAny(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
            Operand::Set(vec![Operand::Integer(50), Operand::Integer(51)]),
        ),
        Condition::ContainsAll(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
            Operand::Set(vec![Operand::Integer(50)]),
        ),
        Condition::HasAttribute(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }),
            s_attr,
        ),
    ]);

    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let mut principal = empty_entity_at(1);
    principal.attributes.set(s_attr,   AttributeValue::String("prefixmidfix".to_string()));
    principal.attributes.set(n_attr,   AttributeValue::Integer(50));
    principal.attributes.set(set_attr, AttributeValue::Set(vec![
        AttributeValue::Integer(49),
        AttributeValue::Integer(50),
    ]));

    let resource = empty_entity_at(2);
    let context = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &context), ConditionResult::True);
}

#[test]
fn test_type_and_hierarchy_integration() {
    let group_idx     = 10u32;
    let principal_idx = 1u32;

    // group entity is self-inclusive (own idx in ancestors)
    let mut group_entity = empty_entity_at(group_idx);
    group_entity.ancestors.insert(group_idx);

    let user_type = EntityTypeId::new(1);
    let resource  = empty_entity_at(2);

    let condition = Condition::And(vec![
        Condition::IsType(VariableScope::Principal, user_type),
        Condition::InHierarchy(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }),
            Operand::EntityRef(group_idx),
        ),
    ]);

    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    // 1. Correct type, in hierarchy → True
    let mut principal = empty_entity_at(principal_idx);
    principal.entity_type = user_type;
    principal.ancestors.insert(group_idx);

    let resolver = MapResolver::new()
        .insert(principal.clone())
        .insert(group_entity.clone());
    let ctx = EvaluationContext::new(&principal, &resource, None, &resolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx), ConditionResult::True);

    // 2. Wrong entity type → False (IsType fails)
    let mut principal_wrong_type = empty_entity_at(principal_idx);
    principal_wrong_type.entity_type = EntityTypeId::new(2);
    principal_wrong_type.ancestors.insert(group_idx);

    let resolver2 = MapResolver::new()
        .insert(principal_wrong_type.clone())
        .insert(group_entity.clone());
    let ctx2 = EvaluationContext::new(&principal_wrong_type, &resource, None, &resolver2);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx2), ConditionResult::False);

    // 3. Right type but not in hierarchy → False (InHierarchy fails)
    let mut principal_not_in_group = empty_entity_at(principal_idx);
    principal_not_in_group.entity_type = user_type;
    // ancestors left empty — group_idx not present

    let resolver3 = MapResolver::new()
        .insert(principal_not_in_group.clone())
        .insert(group_entity.clone());
    let ctx3 = EvaluationContext::new(&principal_not_in_group, &resource, None, &resolver3);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx3), ConditionResult::False);
}

#[test]
fn test_simple_eq_integration() {
    let condition = Condition::Eq(Operand::Integer(42), Operand::Integer(42));
    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let principal = empty_entity_at(1);
    let resource  = empty_entity_at(2);
    let context   = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &context), ConditionResult::True);
}

#[test]
fn test_variable_lookup_integration() {
    let attr_id   = AttributeNameId::new(1);
    let condition = Condition::Eq(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![attr_id] }),
        Operand::String("Alice".to_string()),
    );
    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let resource = empty_entity_at(2);

    let mut principal = empty_entity_at(1);
    principal.attributes.set(attr_id, AttributeValue::String("Alice".to_string()));
    let ctx = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx), ConditionResult::True);

    let mut principal_wrong = empty_entity_at(1);
    principal_wrong.attributes.set(attr_id, AttributeValue::String("Bob".to_string()));
    let ctx_wrong = EvaluationContext::new(&principal_wrong, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx_wrong), ConditionResult::False);
}

#[test]
fn test_short_circuit_and_integration() {
    let condition = Condition::And(vec![
        Condition::Eq(Operand::Bool(true), Operand::Bool(true)),
        Condition::Eq(Operand::Bool(true), Operand::Bool(false)),
    ]);
    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    let principal = empty_entity_at(1);
    let resource  = empty_entity_at(2);
    let context   = EvaluationContext::new(&principal, &resource, None, &NoopResolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &context), ConditionResult::False);
}

/// Tests InHierarchy with an attribute-path variable (EntityRef stored in an attribute).
/// This replaces the old ContainsInHierarchy test.
#[test]
fn test_in_hierarchy_attr_integration() {
    let admin_group_idx = 500u32;
    let sub_group_idx   = 501u32;
    let groups_attr     = AttributeNameId::new(10);
    let resource        = empty_entity_at(2);

    let mut admin_group = empty_entity_at(admin_group_idx);
    admin_group.ancestors.insert(admin_group_idx);

    let mut sub_group = empty_entity_at(sub_group_idx);
    sub_group.ancestors.insert(admin_group_idx);
    sub_group.ancestors.insert(sub_group_idx);

    // Condition: entity referenced by principal.groups_attr is in admin_group's hierarchy
    let condition = Condition::InHierarchy(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![groups_attr] }),
        Operand::EntityRef(admin_group_idx),
    );
    let compiled = BytecodeCompiler::new().compile(&condition).expect("Compilation failed");

    // 1. principal.groups_attr = admin_group directly → True
    let mut principal_direct = empty_entity_at(1);
    principal_direct.attributes.set(groups_attr, AttributeValue::EntityRef(admin_group_idx));
    let resolver = MapResolver::new().insert(admin_group.clone());
    let ctx = EvaluationContext::new(&principal_direct, &resource, None, &resolver);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx), ConditionResult::True);

    // 2. principal.groups_attr = sub_group, which has admin_group as ancestor → True (transitive)
    let mut principal_nested = empty_entity_at(1);
    principal_nested.attributes.set(groups_attr, AttributeValue::EntityRef(sub_group_idx));
    let resolver2 = MapResolver::new()
        .insert(admin_group.clone())
        .insert(sub_group.clone());
    let ctx2 = EvaluationContext::new(&principal_nested, &resource, None, &resolver2);
    assert_eq!(BytecodeVM::new().evaluate(&compiled.instructions, &ctx2), ConditionResult::True);
}
