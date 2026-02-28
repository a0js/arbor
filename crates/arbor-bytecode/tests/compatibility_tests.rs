use arbor_types::{
    AttributeValue, Condition, EntityResolver, IndexedEntity,
    Operand, ScalarValue, VariableRef, VariableScope, AttributeNameId,
    EvaluationContext, ConditionResult, Attributes, EntityTypeId,
};
use arbor_bytecode::compiler::BytecodeCompiler;
use arbor_bytecode::bytecode_vm::BytecodeVM;
use uuid::Uuid;
use std::collections::BTreeMap;
use roaring::RoaringBitmap;

struct MockResolver {
    entities: BTreeMap<u32, IndexedEntity>,
    uuids: BTreeMap<Uuid, u32>,
}

impl EntityResolver for MockResolver {
    fn get_entity(&self, index: u32) -> Option<&IndexedEntity> {
        self.entities.get(&index)
    }
    fn resolve_uuid(&self, uuid: &Uuid) -> Option<u32> {
        self.uuids.get(uuid).copied()
    }
}

fn make_indexed_entity(type_id: u32) -> IndexedEntity {
    IndexedEntity {
        attributes: Attributes::new(),
        entity_type: EntityTypeId::new(type_id),
        descendants: RoaringBitmap::new(),
        ancestors: RoaringBitmap::new(),
        principal_of_policies: None,
        resource_of_policies: None,
    }
}

#[test]
fn test_compiler_vm_compatibility() {
    let mut resolver = MockResolver {
        entities: BTreeMap::new(),
        uuids: BTreeMap::new(),
    };
    
    // Setup some entities for hierarchy tests
    let user_uuid = Uuid::new_v4();
    let group_uuid = Uuid::new_v4();
    resolver.uuids.insert(user_uuid, 1);
    resolver.uuids.insert(group_uuid, 2);
    
    let mut user_entity = make_indexed_entity(100);
    user_entity.ancestors.insert(1);
    user_entity.ancestors.insert(2); // user is in group
    resolver.entities.insert(1, user_entity.clone());
    
    let mut group_entity = make_indexed_entity(101);
    group_entity.ancestors.insert(2);
    resolver.entities.insert(2, group_entity);

    let compiler = BytecodeCompiler::new(&resolver);

    // Test 1: Simple Equality
    let cond_eq = Condition::Eq(
        Operand::Scalar(ScalarValue::Integer(10)),
        Operand::Scalar(ScalarValue::Integer(10)),
    );
    let compiled = compiler.compile(&cond_eq).expect("Failed to compile Eq");
    
    let context = EvaluationContext::new(&user_entity, &user_entity, None);
    let mut vm = BytecodeVM::new(&context);
    let result = vm.evaluate(&compiled.instructions);
    assert!(matches!(result, ConditionResult::True), "Eq(10, 10) should be true");

    // Test 2: Variable Resolution & Comparison
    // principal.age == 30
    let age_attr = AttributeNameId::new(1);
    let mut principal_with_age = user_entity.clone();
    principal_with_age.attributes.set(age_attr, AttributeValue::Scalar(ScalarValue::Integer(30)));
    
    let cond_var = Condition::Eq(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![age_attr] }),
        Operand::Scalar(ScalarValue::Integer(30)),
    );
    let compiled_var = compiler.compile(&cond_var).expect("Failed to compile Var Eq");
    
    let context_var = EvaluationContext::new(&principal_with_age, &user_entity, None);
    let mut vm_var = BytecodeVM::new(&context_var);
    let result_var = vm_var.evaluate(&compiled_var.instructions);
    assert!(matches!(result_var, ConditionResult::True), "principal.age == 30 should be true");

    // Test 3: InHierarchy
    let cond_in = Condition::InHierarchy(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }),
        Operand::EntityRef(group_uuid),
    );
    let compiled_in = compiler.compile(&cond_in).expect("Failed to compile InHierarchy");
    
    let context_in = EvaluationContext::new(&principal_with_age, &user_entity, None).with_entities(&resolver);
    let mut vm_in = BytecodeVM::new(&context_in);
    let result_in = vm_in.evaluate(&compiled_in.instructions);
    assert!(matches!(result_in, ConditionResult::True), "User should be in group hierarchy");

    // Test 4: HasAttribute
    // principal HAS ATTRIBUTE "age"
    let cond_has = Condition::HasAttribute(
        Operand::Variable(VariableRef { scope: VariableScope::Principal, path: vec![] }),
        age_attr,
    );
    let compiled_has = compiler.compile(&cond_has).expect("Failed to compile HasAttribute");
    let mut vm_has = BytecodeVM::new(&context_var);
    let result_has = vm_has.evaluate(&compiled_has.instructions);
    assert!(matches!(result_has, ConditionResult::True), "principal should have age attribute");

    // Test 5: In (Set)
    let cond_set_in = Condition::In(
        Operand::Scalar(ScalarValue::Integer(1)),
        Operand::Set(vec![
            Operand::Scalar(ScalarValue::Integer(1)),
            Operand::Scalar(ScalarValue::Integer(2)),
        ]),
    );
    let compiled_set_in = compiler.compile(&cond_set_in).expect("Failed to compile In Set");
    let mut vm_set_in = BytecodeVM::new(&context);
    let result_set_in = vm_set_in.evaluate(&compiled_set_in.instructions);
    assert!(matches!(result_set_in, ConditionResult::True), "1 should be IN [1, 2]");

    // Test 6: Contains (Set)
    let cond_set_contains = Condition::Contains(
        Operand::Set(vec![
            Operand::Scalar(ScalarValue::Integer(1)),
            Operand::Scalar(ScalarValue::Integer(2)),
        ]),
        Operand::Scalar(ScalarValue::Integer(2)),
    );
    let compiled_set_contains = compiler.compile(&cond_set_contains).expect("Failed to compile Contains Set");
    let mut vm_set_contains = BytecodeVM::new(&context);
    let result_set_contains = vm_set_contains.evaluate(&compiled_set_contains.instructions);
    assert!(matches!(result_set_contains, ConditionResult::True), "[1, 2] should CONTAINS 2");

    // Test 7: String operations
    let cond_starts_with = Condition::StartsWith(
        Operand::Scalar(ScalarValue::String("hello world".into())),
        Operand::Scalar(ScalarValue::String("hello".into())),
    );
    let compiled_starts_with = compiler.compile(&cond_starts_with).expect("Failed to compile StartsWith");
    let mut vm_starts_with = BytecodeVM::new(&context);
    let result_starts_with = vm_starts_with.evaluate(&compiled_starts_with.instructions);
    assert!(matches!(result_starts_with, ConditionResult::True), "'hello world' should start with 'hello'");

    // Test 8: Not
    let cond_not = Condition::Not(Box::new(Condition::Eq(
        Operand::Scalar(ScalarValue::Integer(1)),
        Operand::Scalar(ScalarValue::Integer(2)),
    )));
    let compiled_not = compiler.compile(&cond_not).expect("Failed to compile Not");
    let mut vm_not = BytecodeVM::new(&context);
    let result_not = vm_not.evaluate(&compiled_not.instructions);
    assert!(matches!(result_not, ConditionResult::True), "NOT (1 == 2) should be true");
}
