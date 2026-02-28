use arbor_bytecode::compiler::BytecodeCompiler;
use arbor_bytecode::bytecode_vm::BytecodeVM;
use arbor_types::{
    AttributeNameId, AttributeValue, Attributes, Condition, ConditionResult,
    EntityResolver, EntityTypeId, EvaluationContext, IndexedEntity, Operand, VariableRef, VariableScope,
};
use roaring::RoaringBitmap;
use uuid::Uuid;
use chrono::{Utc, TimeZone};
use std::net::IpAddr;
use ordered_float::OrderedFloat;

struct MockResolver {
    uuids: std::collections::HashMap<Uuid, u32>,
    entities: std::collections::HashMap<u32, IndexedEntity>,
}

impl MockResolver {
    fn new() -> Self {
        Self {
            uuids: std::collections::HashMap::new(),
            entities: std::collections::HashMap::new(),
        }
    }

    fn add_entity(&mut self, uuid: Uuid, idx: u32, entity: IndexedEntity) {
        self.uuids.insert(uuid, idx);
        self.entities.insert(idx, entity);
    }
}

impl EntityResolver for MockResolver {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        self.entities.get(&index)
    }
    fn resolve_uuid(&self, uuid: &Uuid) -> Option<u32> {
        self.uuids.get(uuid).copied()
    }
}

fn empty_entity() -> IndexedEntity {
    IndexedEntity {
        attributes: Attributes::new(),
        entity_type: EntityTypeId::new(0),
        descendants: RoaringBitmap::new(),
        ancestors: RoaringBitmap::new(),
        principal_of_policies: None,
        resource_of_policies: None,
    }
}

#[test]
fn test_all_operand_types_integration() {
    let mut resolver = MockResolver::new();
    let principal_uuid = Uuid::new_v4();
    let principal_idx = 1u32;
    let mut principal = empty_entity();
    
    let float_attr = AttributeNameId::new(1);
    let timestamp_attr = AttributeNameId::new(2);
    let ip_attr = AttributeNameId::new(3);
    let string_attr = AttributeNameId::new(4);
    let int_attr = AttributeNameId::new(5);
    let bool_attr = AttributeNameId::new(6);
    let entity_ref_attr = AttributeNameId::new(7);
    let set_attr = AttributeNameId::new(8);

    let now = Utc.with_ymd_and_hms(2024, 1, 1, 0, 0, 0).unwrap();
    let ip: IpAddr = "192.168.1.1".parse().unwrap();
    let other_uuid = Uuid::new_v4();

    principal.attributes.set(float_attr, AttributeValue::Float(OrderedFloat(1.5)));
    principal.attributes.set(timestamp_attr, AttributeValue::Timestamp(now));
    principal.attributes.set(ip_attr, AttributeValue::IpAddr(ip));
    principal.attributes.set(string_attr, AttributeValue::String("test".to_string()));
    principal.attributes.set(int_attr, AttributeValue::Integer(100));
    principal.attributes.set(bool_attr, AttributeValue::Bool(true));
    principal.attributes.set(entity_ref_attr, AttributeValue::EntityRef(other_uuid));
    principal.attributes.set(set_attr, AttributeValue::Set(vec![
        AttributeValue::Integer(1),
        AttributeValue::Integer(2),
    ]));

    resolver.add_entity(principal_uuid, principal_idx, principal.clone());
    let compiler = BytecodeCompiler::new(&resolver);

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

    let compiled = compiler.compile(&condition).expect("Compilation failed");
    
    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None).with_entities(&resolver);
    let mut vm = BytecodeVM::new(&context);

    let result = vm.evaluate(&compiled.instructions);
    assert_eq!(result, ConditionResult::True);
}

#[test]
fn test_complex_nested_logic_integration() {
    let resolver = MockResolver::new();
    let compiler = BytecodeCompiler::new(&resolver);

    let age_attr = AttributeNameId::new(1);
    let locked_attr = AttributeNameId::new(2);
    let public_attr = AttributeNameId::new(3);
    let role_attr = AttributeNameId::new(4);

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

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let mut principal = empty_entity();
    principal.attributes.set(age_attr, AttributeValue::Integer(25));
    principal.attributes.set(locked_attr, AttributeValue::Bool(false));
    principal.attributes.set(role_attr, AttributeValue::String("user".to_string()));

    let mut resource = empty_entity();
    resource.attributes.set(public_attr, AttributeValue::Bool(false));

    let context = EvaluationContext::new(&principal, &resource, None);
    let mut vm = BytecodeVM::new(&context);
    assert_eq!(vm.evaluate(&compiled.instructions), ConditionResult::True);

    principal.attributes.set(locked_attr, AttributeValue::Bool(true));
    let context2 = EvaluationContext::new(&principal, &resource, None);
    let mut vm2 = BytecodeVM::new(&context2);
    assert_eq!(vm2.evaluate(&compiled.instructions), ConditionResult::True);

    principal.attributes.set(role_attr, AttributeValue::String("admin".to_string()));
    let context3 = EvaluationContext::new(&principal, &resource, None);
    let mut vm3 = BytecodeVM::new(&context3);
    assert_eq!(vm3.evaluate(&compiled.instructions), ConditionResult::False);
}

#[test]
fn test_all_operators_integration() {
    let resolver = MockResolver::new();
    let compiler = BytecodeCompiler::new(&resolver);

    let s_attr = AttributeNameId::new(1);
    let n_attr = AttributeNameId::new(2);
    let set_attr = AttributeNameId::new(3);

    let condition = Condition::And(vec![
        Condition::StartsWith(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }), Operand::String("pre".to_string())),
        Condition::EndsWith(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }), Operand::String("fix".to_string())),
        Condition::StringContains(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }), Operand::String("mid".to_string())),
        Condition::Like(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![s_attr] }), Operand::String("pre*fix".to_string())),
        
        Condition::Neq(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![n_attr] }), Operand::Integer(0)),
        Condition::Lte(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![n_attr] }), Operand::Integer(100)),
        
        Condition::In(Operand::Integer(50), Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] })),
        Condition::Contains(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }), Operand::Integer(50)),
        Condition::ContainsAny(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
            Operand::Set(vec![Operand::Integer(50), Operand::Integer(51)])
        ),
        Condition::ContainsAll(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![set_attr] }),
            Operand::Set(vec![Operand::Integer(50)])
        ),

        Condition::HasAttribute(Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }), s_attr),
    ]);

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let mut principal = empty_entity();
    principal.attributes.set(s_attr, AttributeValue::String("prefixmidfix".to_string()));
    principal.attributes.set(n_attr, AttributeValue::Integer(50));
    principal.attributes.set(set_attr, AttributeValue::Set(vec![
        AttributeValue::Integer(49),
        AttributeValue::Integer(50),
    ]));

    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None);
    let mut vm = BytecodeVM::new(&context);
    assert_eq!(vm.evaluate(&compiled.instructions), ConditionResult::True);
}

#[test]
fn test_type_and_hierarchy_integration() {
    let mut resolver = MockResolver::new();
    let group_uuid = Uuid::new_v4();
    let group_idx = 10u32;
    let mut group_entity = empty_entity();
    group_entity.ancestors.insert(group_idx);
    resolver.add_entity(group_uuid, group_idx, group_entity);

    let user_type = EntityTypeId::new(1);
    let compiler = BytecodeCompiler::new(&resolver);

    let condition = Condition::And(vec![
        Condition::IsType(VariableScope::Principal, user_type),
        Condition::InHierarchy(
            Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }),
            Operand::EntityRef(group_uuid),
        ),
    ]);

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let mut principal = empty_entity();
    principal.entity_type = user_type;
    principal.ancestors.insert(group_idx);

    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None).with_entities(&resolver);
    let mut vm = BytecodeVM::new(&context);
    assert_eq!(vm.evaluate(&compiled.instructions), ConditionResult::True);

    let mut principal_wrong_type = principal.clone();
    principal_wrong_type.entity_type = EntityTypeId::new(2);
    let context2 = EvaluationContext::new(&principal_wrong_type, &resource, None).with_entities(&resolver);
    let mut vm2 = BytecodeVM::new(&context2);
    assert_eq!(vm2.evaluate(&compiled.instructions), ConditionResult::False);

    let mut principal_wrong_hierarchy = principal.clone();
    principal_wrong_hierarchy.ancestors = RoaringBitmap::new();
    let context3 = EvaluationContext::new(&principal_wrong_hierarchy, &resource, None).with_entities(&resolver);
    let mut vm3 = BytecodeVM::new(&context3);
    assert_eq!(vm3.evaluate(&compiled.instructions), ConditionResult::False);
}

#[test]
fn test_simple_eq_integration() {
    let resolver = MockResolver::new();
    let compiler = BytecodeCompiler::new(&resolver);

    let condition = Condition::Eq(
        Operand::Integer(42),
        Operand::Integer(42),
    );

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let principal = empty_entity();
    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None);
    let mut vm = BytecodeVM::new(&context);

    let result = vm.evaluate(&compiled.instructions);
    assert_eq!(result, ConditionResult::True);
}

#[test]
fn test_variable_lookup_integration() {
    let resolver = MockResolver::new();
    let compiler = BytecodeCompiler::new(&resolver);

    let attr_id = AttributeNameId::new(1);
    let condition = Condition::Eq(
        Operand::Variable(VariableRef {
            scope: VariableScope::Principal,
            path: vec![attr_id],
        }),
        Operand::String("Alice".to_string()),
    );

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let mut principal = empty_entity();
    principal.attributes.set(attr_id, AttributeValue::String("Alice".to_string()));
    
    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None);
    let mut vm = BytecodeVM::new(&context);

    let result = vm.evaluate(&compiled.instructions);
    assert_eq!(result, ConditionResult::True);

    let mut principal_wrong = empty_entity();
    principal_wrong.attributes.set(attr_id, AttributeValue::String("Bob".to_string()));
    let context_wrong = EvaluationContext::new(&principal_wrong, &resource, None);
    let mut vm_wrong = BytecodeVM::new(&context_wrong);
    let result_wrong = vm_wrong.evaluate(&compiled.instructions);
    assert_eq!(result_wrong, ConditionResult::False);
}

#[test]
fn test_short_circuit_and_integration() {
    let resolver = MockResolver::new();
    let compiler = BytecodeCompiler::new(&resolver);

    let condition = Condition::And(vec![
        Condition::Eq(Operand::Bool(true), Operand::Bool(true)),
        Condition::Eq(Operand::Bool(true), Operand::Bool(false)),
    ]);

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let principal = empty_entity();
    let resource = empty_entity();
    let context = EvaluationContext::new(&principal, &resource, None);
    let mut vm = BytecodeVM::new(&context);

    let result = vm.evaluate(&compiled.instructions);
    assert_eq!(result, ConditionResult::False);
}

#[test]
fn test_contains_in_hierarchy_integration() {
    let admin_group_uuid = Uuid::new_v4();
    let admin_group_idx = 500u32;
    
    let mut resolver = MockResolver::new();
    resolver.uuids.insert(admin_group_uuid, admin_group_idx);
    
    let compiler = BytecodeCompiler::new(&resolver);

    let groups_attr = AttributeNameId::new(10);
    let condition = Condition::ContainsInHierarchy(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![groups_attr] }),
        Operand::EntityRef(admin_group_uuid),
    );

    let compiled = compiler.compile(&condition).expect("Compilation failed");

    let mut admin_group_entity = empty_entity();
    admin_group_entity.ancestors.insert(admin_group_idx);
    resolver.add_entity(admin_group_uuid, admin_group_idx, admin_group_entity);

    let mut principal_direct = empty_entity();
    principal_direct.attributes.set(groups_attr, AttributeValue::Set(vec![
        AttributeValue::EntityRef(admin_group_uuid)
    ]));
    
    let resource = empty_entity();
    let context_direct = EvaluationContext::new(&principal_direct, &resource, None)
        .with_entities(&resolver);
    let mut vm = BytecodeVM::new(&context_direct);
    assert_eq!(vm.evaluate(&compiled.instructions), ConditionResult::True);

    let sub_group_uuid = Uuid::new_v4();
    let sub_group_idx = 501u32;
    
    let mut sub_group_entity = empty_entity();
    sub_group_entity.ancestors.insert(admin_group_idx);
    sub_group_entity.ancestors.insert(sub_group_idx);

    resolver.add_entity(sub_group_uuid, sub_group_idx, sub_group_entity);
    
    let mut principal_nested = empty_entity();
    principal_nested.attributes.set(groups_attr, AttributeValue::Set(vec![
        AttributeValue::EntityRef(sub_group_uuid)
    ]));
    
    let context_nested = EvaluationContext::new(&principal_nested, &resource, None)
        .with_entities(&resolver);
    let mut vm_nested = BytecodeVM::new(&context_nested);
    assert_eq!(vm_nested.evaluate(&compiled.instructions), ConditionResult::True);
}
