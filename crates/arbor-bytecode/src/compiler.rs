//! Bytecode compiler: translates a [`Condition`] AST into a [`Vec<OpCode>`].
//!
//! The compiler performs two jobs:
//!
//! 1. Structural translation — each `Condition` variant maps to one or more
//!    `OpCode` instructions following the stack discipline the VM expects.
//! 2. UUID resolution — hierarchy opcodes (`InHierarchy`, `ContainsInHierarchy`)
//!    require a snapshot index (`u32`) rather than a UUID.  The compiler asks the
//!    supplied [`EntityResolver`] to perform this lookup at compile time so the VM
//!    never touches UUIDs at evaluation time.
//!
//! ## Stack discipline
//!
//! The VM is a pure stack machine that pops right-to-left: for binary opcodes it
//! pops the *right* operand first, then the *left*.  Therefore, the compiler always
//! pushes LEFT then RIGHT before emitting a binary opcode.
//!
//! ## Example
//!
//! ```rust
//! use std::collections::HashMap;
//! use uuid::Uuid;
//! use arbor_types::{
//!     Condition, Operand, AttributeValue, OpCode, EntityResolver, IndexedEntity, CompileError
//! };
//! use arbor_bytecode::compiler::BytecodeCompiler;
//!
//! struct NoopResolver;
//! impl EntityResolver for NoopResolver {
//!     fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
//!     fn resolve_uuid(&self, _: &Uuid) -> Option<u32> { None }
//! }
//!
//! let resolver = NoopResolver;
//! let compiler = BytecodeCompiler::new(&resolver);
//! let condition = Condition::Eq(
//!     Operand::Integer(1),
//!     Operand::Integer(1),
//! );
//! let compiled = compiler.compile(&condition).unwrap();
//! assert_eq!(compiled.instructions.len(), 3); // PushScalar, PushScalar, Eq
//! ```

use arbor_types::{AttributeValue, Condition, CompiledCondition, CompileWarning, EntityResolver, OpCode, Operand, VariableRef, VariableScope, CompileError};
use uuid::Uuid;
use tracing::warn;

// ── Compiler ─────────────────────────────────────────────────────────────────

/// Translates a [`Condition`] AST into a [`CompiledCondition`] (bytecode +
/// dependency list).
///
/// The compiler borrows an [`EntityResolver`] for the duration of compilation
/// so it can resolve entity UUIDs to snapshot indices for hierarchy opcodes.
///
/// # Example
///
/// ```rust
/// use uuid::Uuid;
/// use arbor_types::{
///     Condition, Operand, AttributeValue, VariableRef, VariableScope,
///     OpCode, EntityResolver, IndexedEntity,
/// };
/// use arbor_bytecode::compiler::BytecodeCompiler;
///
/// struct NoopResolver;
/// impl EntityResolver for NoopResolver {
///     fn get_entity(&self, _: u32) -> Option<&IndexedEntity> { None }
///     fn resolve_uuid(&self, _: &Uuid) -> Option<u32> { None }
/// }
///
/// let resolver = NoopResolver;
/// let compiler = BytecodeCompiler::new(&resolver);
///
/// let condition = Condition::And(vec![
///     Condition::Eq(
///         Operand::Bool(true),
///         Operand::Bool(true),
///     ),
/// ]);
/// let compiled = compiler.compile(&condition).unwrap();
/// // Single-element And emits no And opcode — just the inner instructions.
/// assert_eq!(compiled.instructions.len(), 3); // PushScalar, PushScalar, Eq
/// ```
pub struct BytecodeCompiler<'a> {
    resolver: &'a dyn EntityResolver,
}

impl<'a> BytecodeCompiler<'a> {
    /// Create a new compiler backed by the given entity resolver.
    #[must_use]
    pub fn new(resolver: &'a dyn EntityResolver) -> Self {
        Self { resolver }
    }

    /// Compile `condition` to a [`CompiledCondition`].
    ///
    /// # Errors
    ///
    /// Returns [`CompileError`] if:
    /// - A referenced entity UUID cannot be resolved (hierarchy ops).
    /// - An unsupported operation is encountered (`InNetwork`).
    /// - An operand is structurally invalid for its enclosing condition.
    pub fn compile(&self, condition: &Condition) -> Result<CompiledCondition, CompileError> {
        let mut instructions = Vec::new();
        let mut warnings = Vec::new();
        self.compile_condition(&mut instructions, &mut warnings, condition)?;
        let dependencies = condition.compute_dependencies();
        Ok(CompiledCondition { instructions, dependencies, warnings })
    }

    // ── Private: recursive condition compiler ────────────────────────────────

    fn compile_condition(
        &self,
        ops: &mut Vec<OpCode>,
        warnings: &mut Vec<CompileWarning>,
        condition: &Condition,
    ) -> Result<(), CompileError> {
        match condition {
            // --- Bare operand ---------------------------------------------------
            Condition::Operand(op) => {
                match op {
                    Operand::Bool(b) => ops.push(OpCode::PushBool(*b)),
                    _ => return Err(CompileError::InvalidOperand(
                        "Bare operand in Condition must be Bool".into(),
                    )),
                }
            }

            // --- Logical --------------------------------------------------------
            Condition::And(conds) => self.compile_and(ops, warnings, conds)?,
            Condition::Or(conds) => self.compile_or(ops, warnings, conds)?,
            Condition::Not(inner) => {
                self.compile_condition(ops, warnings, inner)?;
                ops.push(OpCode::Not);
            }

            // --- Comparisons ----------------------------------------------------
            Condition::Eq(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Eq);
            }
            Condition::Neq(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Neq);
            }
            Condition::Lt(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Lt);
            }
            Condition::Lte(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Lte);
            }
            Condition::Gt(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Gt);
            }
            Condition::Gte(l, r) => {
                self.compile_binary(ops, l, r)?;
                ops.push(OpCode::Gte);
            }

            // --- Set operations -------------------------------------------------
            // VM execute_in:       pops set (top), then element → push elem first
            Condition::In(elem, set) => {
                self.compile_operand(ops, elem)?;
                self.compile_operand(ops, set)?;
                ops.push(OpCode::In);
            }
            // VM execute_contains: pops element (top), then set → push set first
            Condition::Contains(set, elem) => {
                self.compile_operand(ops, set)?;
                self.compile_operand(ops, elem)?;
                ops.push(OpCode::Contains);
            }
            // VM execute_contains_all: pops subset (top), then set → push set first
            Condition::ContainsAll(set, subset) => {
                self.compile_operand(ops, set)?;
                self.compile_operand(ops, subset)?;
                ops.push(OpCode::ContainsAll);
            }
            // VM execute_contains_any: pops subset (top), then set → push set first
            Condition::ContainsAny(set, subset) => {
                self.compile_operand(ops, set)?;
                self.compile_operand(ops, subset)?;
                ops.push(OpCode::ContainsAny);
            }

            // --- String operations ----------------------------------------------
            // VM pops right (pattern/suffix/needle) first, then left (string)
            Condition::StartsWith(s, prefix) => {
                self.compile_binary(ops, s, prefix)?;
                ops.push(OpCode::StartsWith);
            }
            Condition::EndsWith(s, suffix) => {
                self.compile_binary(ops, s, suffix)?;
                ops.push(OpCode::EndsWith);
            }
            Condition::StringContains(haystack, needle) => {
                self.compile_binary(ops, haystack, needle)?;
                ops.push(OpCode::StringContains);
            }
            Condition::Like(s, pattern) => {
                self.compile_binary(ops, s, pattern)?;
                ops.push(OpCode::Like);
            }

            // --- Attribute existence --------------------------------------------
            Condition::HasAttribute(op, attr_name) => {
                match op {
                    Operand::Variable(var_ref) => {
                        // Append attr_name to the path to form the full lookup path.
                        let mut full_path = var_ref.path.clone();
                        full_path.push(*attr_name);
                        ops.push(OpCode::HasAttribute(VariableRef {
                            scope: var_ref.scope.clone(),
                            path: full_path,
                        }));
                    }
                    _ => {
                        return Err(CompileError::InvalidOperand(
                            "HasAttribute requires a Variable operand".into(),
                        ));
                    }
                }
            }

            // --- Type check -----------------------------------------------------
            Condition::IsType(scope, type_id) => {
                ops.push(OpCode::IsType(scope.clone(), *type_id));
            }

            // --- Hierarchy ------------------------------------------------------
            Condition::InHierarchy(left, right) => {
                self.compile_in_hierarchy(ops, warnings, left, right)?;
            }
            Condition::ContainsInHierarchy(left, right) => {
                self.compile_contains_in_hierarchy(ops, warnings, left, right)?;
            }

            // --- IP network membership ------------------------------------------
            Condition::InNetwork(left, right) => {
                self.compile_in_network(ops, left, right)?;
            }
        }
        Ok(())
    }

    // ── Logical helpers ──────────────────────────────────────────────────────

    fn compile_and(
        &self,
        ops: &mut Vec<OpCode>,
        warnings: &mut Vec<CompileWarning>,
        conds: &[Condition],
    ) -> Result<(), CompileError> {
        match conds.len() {
            0 => {
                ops.push(OpCode::PushBool(true));
            }
            1 => {
                self.compile_condition(ops, warnings, &conds[0])?;
            }
            _ => {
                // Emit each non-last condition with a JumpIfFalse placeholder
                let mut false_patches = Vec::with_capacity(conds.len() - 1);
                for cond in &conds[..conds.len() - 1] {
                    self.compile_condition(ops, warnings, cond)?;
                    false_patches.push(ops.len());
                    ops.push(OpCode::JumpIfFalse(0)); // placeholder
                }

                // Last condition: its result is the final result if all prior were true
                self.compile_condition(ops, warnings, conds.last().unwrap())?;

                // Jump over the false_label
                let end_patch = ops.len();
                ops.push(OpCode::Jump(0)); // placeholder

                // false_label: short-circuit result
                let false_label = ops.len() as u32;
                ops.push(OpCode::PushBool(false));

                // end: (index after the PushScalar)
                let end = ops.len() as u32;

                // Backpatch all JumpIfFalse to false_label
                for idx in false_patches {
                    ops[idx] = OpCode::JumpIfFalse(false_label);
                }
                // Backpatch the unconditional Jump to end
                ops[end_patch] = OpCode::Jump(end);
            }
        }
        Ok(())
    }

    fn compile_or(
        &self,
        ops: &mut Vec<OpCode>,
        warnings: &mut Vec<CompileWarning>,
        conds: &[Condition],
    ) -> Result<(), CompileError> {
        match conds.len() {
            0 => {
                ops.push(OpCode::PushBool(false));
            }
            1 => {
                self.compile_condition(ops, warnings, &conds[0])?;
            }
            _ => {
                let mut true_patches = Vec::with_capacity(conds.len() - 1);
                for cond in &conds[..conds.len() - 1] {
                    self.compile_condition(ops, warnings, cond)?;
                    true_patches.push(ops.len());
                    ops.push(OpCode::JumpIfTrue(0)); // placeholder
                }

                // Last condition — result flows through directly
                self.compile_condition(ops, warnings, conds.last().unwrap())?;

                let end_patch = ops.len();
                ops.push(OpCode::Jump(0)); // placeholder

                let true_label = ops.len() as u32;
                ops.push(OpCode::PushBool(true));

                let end = ops.len() as u32;

                for idx in true_patches {
                    ops[idx] = OpCode::JumpIfTrue(true_label);
                }
                ops[end_patch] = OpCode::Jump(end);
            }
        }
        Ok(())
    }

    // ── Operand helpers ──────────────────────────────────────────────────────

    /// Push left then right operand — matches VM pop order (right first, left
    /// second) for every standard binary opcode.
    fn compile_binary(
        &self,
        ops: &mut Vec<OpCode>,
        left: &Operand,
        right: &Operand,
    ) -> Result<(), CompileError> {
        self.compile_operand(ops, left)?;
        self.compile_operand(ops, right)?;
        Ok(())
    }

    fn compile_operand(
        &self,
        ops: &mut Vec<OpCode>,
        operand: &Operand,
    ) -> Result<(), CompileError> {
        match operand {
            Operand::String(s) => ops.push(OpCode::PushString(s.clone())),
            Operand::Integer(i) => ops.push(OpCode::PushInteger(*i)),
            Operand::Float(f) => ops.push(OpCode::PushFloat(*f)),
            Operand::Bool(b) => ops.push(OpCode::PushBool(*b)),
            Operand::Timestamp(t) => ops.push(OpCode::PushTimestamp(*t)),
            Operand::IpAddr(ip) => ops.push(OpCode::PushIpAddr(*ip)),
            Operand::IpNetwork(net) => ops.push(OpCode::PushIpNetwork(*net)),
            Operand::EntityRef(uuid) => ops.push(OpCode::PushEntityRef(*uuid)),
            Operand::Set(items) => {
                let avs = items
                    .iter()
                    .map(Self::operand_to_av)
                    .collect::<Result<Vec<_>, _>>()?;
                ops.push(OpCode::PushSet(avs));
            }
            Operand::Variable(var_ref) => ops.push(OpCode::PushVariable(var_ref.clone())),
        }
        Ok(())
    }

    /// Convert a set-literal operand to an [`AttributeValue`].
    ///
    /// Variables are not allowed inside set literals because the VM has no
    /// mechanism to resolve them there; the compiler rejects them early.
    fn operand_to_av(operand: &Operand) -> Result<AttributeValue, CompileError> {
        match operand {
            Operand::String(s) => Ok(AttributeValue::String(s.clone())),
            Operand::Integer(i) => Ok(AttributeValue::Integer(*i)),
            Operand::Float(f) => Ok(AttributeValue::Float(*f)),
            Operand::Bool(b) => Ok(AttributeValue::Bool(*b)),
            Operand::Timestamp(t) => Ok(AttributeValue::Timestamp(*t)),
            Operand::IpAddr(ip) => Ok(AttributeValue::IpAddr(*ip)),
            Operand::IpNetwork(net) => Ok(AttributeValue::IpNetwork(*net)),
            Operand::EntityRef(uuid) => Ok(AttributeValue::EntityRef(*uuid)),
            Operand::Set(items) => {
                let avs = items
                    .iter()
                    .map(Self::operand_to_av)
                    .collect::<Result<Vec<_>, _>>()?;
                Ok(AttributeValue::Set(avs))
            }
            Operand::Variable(_) => Err(CompileError::InvalidOperand(
                "Variable cannot appear inside a set literal".into(),
            )),
        }
    }

    // ── Hierarchy helpers ────────────────────────────────────────────────────

    fn compile_in_hierarchy(
        &self,
        ops: &mut Vec<OpCode>,
        warnings: &mut Vec<CompileWarning>,
        left: &Operand,
        right: &Operand,
    ) -> Result<(), CompileError> {
        // Right operand must be an EntityRef. If the UUID isn't in the current
        // snapshot (e.g. policy was written before the entity was created),
        // emit a constant false — no entity can be in the hierarchy of
        // something that doesn't exist. The policy is still indexed and will
        // evaluate correctly once the entity appears in a future snapshot.
        let uuid = self.extract_entity_ref_uuid(right)?;
        let Some(target_idx) = self.resolver.resolve_uuid(&uuid) else {
            warn!(
                uuid = %uuid,
                "InHierarchy: entity UUID not found in snapshot; \
                 compiling to constant false — policy will re-evaluate \
                 correctly once the entity is indexed"
            );
            warnings.push(CompileWarning::UnresolvedEntityRef(uuid));
            ops.push(OpCode::PushBool(false));
            return Ok(());
        };

        match left {
            Operand::Variable(var_ref) => {
                if var_ref.path.is_empty() {
                    // Root scope: use the fast InHierarchy opcode (reads entity
                    // directly from context — no stack push).
                    match var_ref.scope {
                        VariableScope::Principal | VariableScope::Resource => {
                            ops.push(OpCode::InHierarchy(var_ref.scope.clone(), target_idx));
                        }
                        VariableScope::Context => {
                            return Err(CompileError::InvalidOperand(
                                "InHierarchy on Context scope with empty path is not valid"
                                    .into(),
                            ));
                        }
                    }
                } else {
                    // Attribute path: use InHierarchyVar to resolve the entity
                    // ref stored at the given attribute path.
                    ops.push(OpCode::InHierarchyVar(var_ref.clone(), target_idx));
                }
            }
            _ => {
                return Err(CompileError::InvalidOperand(
                    "InHierarchy left operand must be a Variable".into(),
                ));
            }
        }
        Ok(())
    }

    fn compile_contains_in_hierarchy(
        &self,
        ops: &mut Vec<OpCode>,
        warnings: &mut Vec<CompileWarning>,
        left: &Operand,
        right: &Operand,
    ) -> Result<(), CompileError> {
        // Same graceful degradation as compile_in_hierarchy: unresolvable UUID
        // → constant false, policy stays in the snapshot.
        let uuid = self.extract_entity_ref_uuid(right)?;
        let Some(target_idx) = self.resolver.resolve_uuid(&uuid) else {
            warn!(
                uuid = %uuid,
                "ContainsInHierarchy: entity UUID not found in snapshot; \
                 compiling to constant false — policy will re-evaluate \
                 correctly once the entity is indexed"
            );
            warnings.push(CompileWarning::UnresolvedEntityRef(uuid));
            ops.push(OpCode::PushBool(false));
            return Ok(());
        };

        // Push the set (left operand) onto the stack.
        match left {
            Operand::Variable(_) | Operand::Set(_) => {
                self.compile_operand(ops, left)?;
            }
            _ => {
                return Err(CompileError::InvalidOperand(
                    "ContainsInHierarchy left operand must be a Variable or Set".into(),
                ));
            }
        }

        ops.push(OpCode::ContainsInHierarchy(target_idx));
        Ok(())
    }

    fn compile_in_network(
        &self,
        ops: &mut Vec<OpCode>,
        left: &Operand,
        right: &Operand,
    ) -> Result<(), CompileError> {
        match left {
            Operand::IpAddr(_) | Operand::Variable(_) => {}
            _ => return Err(CompileError::InvalidOperand(
                "InNetwork left operand must be an IpAddr or Variable".into(),
            )),
        }
        match right {
            Operand::IpNetwork(_) | Operand::Variable(_) => {}
            _ => return Err(CompileError::InvalidOperand(
                "InNetwork right operand must be an IpNetwork or Variable".into(),
            )),
        }
        self.compile_operand(ops, left)?;
        self.compile_operand(ops, right)?;
        ops.push(OpCode::InNetwork);
        Ok(())
    }

    /// Extract the UUID from an `Operand::EntityRef`.
    ///
    /// Returns `InvalidOperand` when the operand is not an `EntityRef`.
    /// Callers are responsible for resolving the UUID to a snapshot index and
    /// deciding what to emit when resolution fails.
    fn extract_entity_ref_uuid(&self, operand: &Operand) -> Result<Uuid, CompileError> {
        match operand {
            Operand::EntityRef(uuid) => Ok(*uuid),
            _ => Err(CompileError::InvalidOperand(
                "hierarchy right operand must be an EntityRef".into(),
            )),
        }
    }
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use arbor_types::{
        AttributeNameId, Condition, CompileWarning, EntityTypeId, IndexedEntity, Operand, OpCode,
        VariableRef, VariableScope,
    };
    use std::collections::HashMap;
    use uuid::Uuid;

    // ── Mock resolver ────────────────────────────────────────────────────────

    struct MockResolver {
        map: HashMap<Uuid, u32>,
    }

    impl MockResolver {
        fn new(entries: Vec<(Uuid, u32)>) -> Self {
            Self { map: entries.into_iter().collect() }
        }

        fn empty() -> Self {
            Self::new(vec![])
        }
    }

    impl EntityResolver for MockResolver {
        fn get_entity(&self, _: u32) -> Option<&IndexedEntity> {
            None
        }

        fn resolve_uuid(&self, uuid: &Uuid) -> Option<u32> {
            self.map.get(uuid).copied()
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────────

    fn compile(condition: &Condition) -> Result<Vec<OpCode>, CompileError> {
        let resolver = MockResolver::empty();
        BytecodeCompiler::new(&resolver).compile(condition).map(|c| c.instructions)
    }

    fn compile_with(
        condition: &Condition,
        resolver: &MockResolver,
    ) -> Result<Vec<OpCode>, CompileError> {
        BytecodeCompiler::new(resolver).compile(condition).map(|c| c.instructions)
    }

    fn compile_full(
        condition: &Condition,
        resolver: &MockResolver,
    ) -> Result<arbor_types::CompiledCondition, CompileError> {
        BytecodeCompiler::new(resolver).compile(condition)
    }

    fn attr(n: u32) -> AttributeNameId {
        AttributeNameId::new(n)
    }

    fn type_id(n: u32) -> EntityTypeId {
        EntityTypeId::new(n)
    }

    fn var(scope: VariableScope, path: &[AttributeNameId]) -> VariableRef {
        VariableRef { scope, path: path.to_vec() }
    }

    fn principal_var() -> Operand {
        Operand::Variable(var(VariableScope::Principal, &[]))
    }

    fn resource_var() -> Operand {
        Operand::Variable(var(VariableScope::Resource, &[]))
    }

    fn scalar_int(n: i64) -> Operand {
        Operand::Integer(n)
    }

    fn scalar_str(s: &str) -> Operand {
        Operand::String(s.to_owned())
    }

    fn scalar_bool(b: bool) -> Operand {
        Operand::Bool(b)
    }

    // ── 1. Simple comparisons ────────────────────────────────────────────────

    #[test]
    fn eq_scalars() {
        let ops = compile(&Condition::Eq(scalar_int(1), scalar_int(2))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(2),
            OpCode::Eq,
        ]);
    }

    #[test]
    fn neq_scalars() {
        let ops = compile(&Condition::Neq(scalar_int(3), scalar_int(4))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(3),
            OpCode::PushInteger(4),
            OpCode::Neq,
        ]);
    }

    #[test]
    fn lt_scalars() {
        let ops = compile(&Condition::Lt(scalar_int(1), scalar_int(2))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(2),
            OpCode::Lt,
        ]);
    }

    #[test]
    fn lte_scalars() {
        let ops = compile(&Condition::Lte(scalar_int(1), scalar_int(2))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(2),
            OpCode::Lte,
        ]);
    }

    #[test]
    fn gt_scalars() {
        let ops = compile(&Condition::Gt(scalar_int(5), scalar_int(3))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(5),
            OpCode::PushInteger(3),
            OpCode::Gt,
        ]);
    }

    #[test]
    fn gte_scalars() {
        let ops = compile(&Condition::Gte(scalar_int(5), scalar_int(5))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(5),
            OpCode::PushInteger(5),
            OpCode::Gte,
        ]);
    }

    #[test]
    fn eq_with_variable() {
        let var_op = Operand::Variable(var(VariableScope::Principal, &[attr(1)]));
        let ops = compile(&Condition::Eq(var_op.clone(), scalar_int(42))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushVariable(var(VariableScope::Principal, &[attr(1)])),
            OpCode::PushInteger(42),
            OpCode::Eq,
        ]);
    }

    // ── 2. Logical ───────────────────────────────────────────────────────────

    #[test]
    fn and_empty() {
        let ops = compile(&Condition::And(vec![])).unwrap();
        assert_eq!(ops, vec![OpCode::PushBool(true)]);
    }

    #[test]
    fn or_empty() {
        let ops = compile(&Condition::Or(vec![])).unwrap();
        assert_eq!(ops, vec![OpCode::PushBool(false)]);
    }

    #[test]
    fn and_single() {
        let ops = compile(&Condition::And(vec![
            Condition::Eq(scalar_int(1), scalar_int(1)),
        ])).unwrap();
        // Single element — no And opcode emitted.
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(1),
            OpCode::Eq,
        ]);
    }

    #[test]
    fn or_single() {
        let ops = compile(&Condition::Or(vec![
            Condition::Eq(scalar_bool(true), scalar_bool(true)),
        ])).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushBool(true),
            OpCode::PushBool(true),
            OpCode::Eq,
        ]);
    }

    #[test]
    fn and_multiple() {
        let ops = compile(&Condition::And(vec![
            Condition::Eq(scalar_int(1), scalar_int(1)),
            Condition::Eq(scalar_int(2), scalar_int(2)),
            Condition::Eq(scalar_int(3), scalar_int(3)),
        ])).unwrap();
        let expected = vec![
            // a
            OpCode::PushInteger(1),
            OpCode::PushInteger(1),
            OpCode::Eq,
            OpCode::JumpIfFalse(12),
            // b
            OpCode::PushInteger(2),
            OpCode::PushInteger(2),
            OpCode::Eq,
            OpCode::JumpIfFalse(12),
            // c
            OpCode::PushInteger(3),
            OpCode::PushInteger(3),
            OpCode::Eq,
            OpCode::Jump(13),
            // false label
            OpCode::PushBool(false),
        ];
        assert_eq!(ops, expected);
    }

    #[test]
    fn or_multiple() {
        let ops = compile(&Condition::Or(vec![
            Condition::Eq(scalar_bool(true), scalar_bool(false)),
            Condition::Eq(scalar_bool(false), scalar_bool(true)),
        ])).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushBool(true),
            OpCode::PushBool(false),
            OpCode::Eq,
            OpCode::JumpIfTrue(8),
            OpCode::PushBool(false),
            OpCode::PushBool(true),
            OpCode::Eq,
            OpCode::Jump(9),
            OpCode::PushBool(true),
        ]);
    }

    #[test]
    fn not() {
        let ops = compile(&Condition::Not(Box::new(Condition::Eq(
            scalar_bool(true),
            scalar_bool(false),
        )))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushBool(true),
            OpCode::PushBool(false),
            OpCode::Eq,
            OpCode::Not,
        ]);
    }

    // ── 3. Set operations ────────────────────────────────────────────────────

    #[test]
    fn in_elem_set() {
        let set = Operand::Set(vec![scalar_int(1), scalar_int(2)]);
        let ops = compile(&Condition::In(scalar_int(1), set)).unwrap();
        // VM execute_in: stack [element, set] — pops set first, then element.
        assert_eq!(ops[0], OpCode::PushInteger(1));
        assert!(matches!(ops[1], OpCode::PushSet(_)));
        assert_eq!(ops[2], OpCode::In);
    }

    #[test]
    fn contains_set_elem() {
        let set = Operand::Set(vec![scalar_int(10), scalar_int(20)]);
        let ops = compile(&Condition::Contains(set, scalar_int(10))).unwrap();
        // VM execute_contains: stack [set, element] — pops element first, then set.
        assert!(matches!(ops[0], OpCode::PushSet(_)));
        assert_eq!(ops[1], OpCode::PushInteger(10));
        assert_eq!(ops[2], OpCode::Contains);
    }

    #[test]
    fn contains_all() {
        let set = Operand::Set(vec![scalar_int(1), scalar_int(2), scalar_int(3)]);
        let subset = Operand::Set(vec![scalar_int(1), scalar_int(2)]);
        let ops = compile(&Condition::ContainsAll(set, subset)).unwrap();
        assert!(matches!(ops[0], OpCode::PushSet(_)));
        assert!(matches!(ops[1], OpCode::PushSet(_)));
        assert_eq!(ops[2], OpCode::ContainsAll);
    }

    #[test]
    fn contains_any() {
        let set = Operand::Set(vec![scalar_int(5), scalar_int(6)]);
        let subset = Operand::Set(vec![scalar_int(6), scalar_int(7)]);
        let ops = compile(&Condition::ContainsAny(set, subset)).unwrap();
        assert!(matches!(ops[0], OpCode::PushSet(_)));
        assert!(matches!(ops[1], OpCode::PushSet(_)));
        assert_eq!(ops[2], OpCode::ContainsAny);
    }

    // ── 4. String operations ─────────────────────────────────────────────────

    #[test]
    fn starts_with() {
        let ops = compile(&Condition::StartsWith(
            scalar_str("hello world"),
            scalar_str("hello"),
        )).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello".into()),
            OpCode::StartsWith,
        ]);
    }

    #[test]
    fn ends_with() {
        let ops = compile(&Condition::EndsWith(
            scalar_str("hello world"),
            scalar_str("world"),
        )).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushString("hello world".into()),
            OpCode::PushString("world".into()),
            OpCode::EndsWith,
        ]);
    }

    #[test]
    fn string_contains() {
        let ops = compile(&Condition::StringContains(
            scalar_str("hello world"),
            scalar_str("lo wo"),
        )).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushString("hello world".into()),
            OpCode::PushString("lo wo".into()),
            OpCode::StringContains,
        ]);
    }

    #[test]
    fn like_glob() {
        let ops = compile(&Condition::Like(
            scalar_str("hello world"),
            scalar_str("hello*"),
        )).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushString("hello world".into()),
            OpCode::PushString("hello*".into()),
            OpCode::Like,
        ]);
    }

    // ── 5. HasAttribute ──────────────────────────────────────────────────────

    #[test]
    fn has_attribute_valid() {
        let base_var = Operand::Variable(var(VariableScope::Principal, &[attr(10)]));
        let ops = compile(&Condition::HasAttribute(base_var, attr(20))).unwrap();
        // Path should be [10, 20].
        assert_eq!(ops, vec![
            OpCode::HasAttribute(var(VariableScope::Principal, &[attr(10), attr(20)])),
        ]);
    }

    #[test]
    fn has_attribute_empty_path() {
        // Variable with empty path — attr_name appended as sole element.
        let base_var = Operand::Variable(var(VariableScope::Resource, &[]));
        let ops = compile(&Condition::HasAttribute(base_var, attr(5))).unwrap();
        assert_eq!(ops, vec![
            OpCode::HasAttribute(var(VariableScope::Resource, &[attr(5)])),
        ]);
    }

    #[test]
    fn has_attribute_invalid_non_variable() {
        let err = compile(&Condition::HasAttribute(scalar_int(1), attr(1))).unwrap_err();
        assert_eq!(
            err,
            CompileError::InvalidOperand("HasAttribute requires a Variable operand".into())
        );
    }

    #[test]
    fn has_attribute_invalid_entity_ref() {
        let err = compile(&Condition::HasAttribute(
            Operand::EntityRef(Uuid::new_v4()),
            attr(1),
        )).unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    // ── 6. IsType ────────────────────────────────────────────────────────────

    #[test]
    fn is_type_no_stack_push() {
        let tid = type_id(42);
        let ops = compile(&Condition::IsType(VariableScope::Principal, tid)).unwrap();
        // IsType reads from context directly — single opcode, no push before it.
        assert_eq!(ops, vec![OpCode::IsType(VariableScope::Principal, tid)]);
    }

    #[test]
    fn is_type_resource_scope() {
        let tid = type_id(7);
        let ops = compile(&Condition::IsType(VariableScope::Resource, tid)).unwrap();
        assert_eq!(ops, vec![OpCode::IsType(VariableScope::Resource, tid)]);
    }

    // ── 7. InHierarchy bare (empty path) ────────────────────────────────────

    #[test]
    fn in_hierarchy_principal_bare() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 5)]);
        let cond = Condition::InHierarchy(
            principal_var(),
            Operand::EntityRef(entity_uuid),
        );
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![OpCode::InHierarchy(VariableScope::Principal, 5)]);
    }

    #[test]
    fn in_hierarchy_resource_bare() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 99)]);
        let cond = Condition::InHierarchy(
            resource_var(),
            Operand::EntityRef(entity_uuid),
        );
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![OpCode::InHierarchy(VariableScope::Resource, 99)]);
    }

    // ── 8. InHierarchy with path ─────────────────────────────────────────────

    #[test]
    fn in_hierarchy_var_with_path() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 12)]);
        let path_var = Operand::Variable(var(VariableScope::Principal, &[attr(1), attr(2)]));
        let cond = Condition::InHierarchy(path_var, Operand::EntityRef(entity_uuid));
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![
            OpCode::InHierarchyVar(
                var(VariableScope::Principal, &[attr(1), attr(2)]),
                12,
            ),
        ]);
    }

    // ── 9. InHierarchy unresolved UUID ───────────────────────────────────────

    #[test]
    fn in_hierarchy_unresolved_uuid_compiles_to_false() {
        // A policy may reference an entity that doesn't exist in the snapshot
        // yet (e.g. written before the group was created). The compiler emits
        // a constant `false` so the policy is still indexed and starts working
        // the moment the entity appears in a future snapshot.
        let unknown_uuid = Uuid::new_v4();
        let resolver = MockResolver::empty();
        let cond = Condition::InHierarchy(
            principal_var(),
            Operand::EntityRef(unknown_uuid),
        );
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![OpCode::PushBool(false)]);
    }

    #[test]
    fn in_hierarchy_unresolved_uuid_produces_warning() {
        let unknown_uuid = Uuid::new_v4();
        let resolver = MockResolver::empty();
        let cond = Condition::InHierarchy(
            principal_var(),
            Operand::EntityRef(unknown_uuid),
        );
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert_eq!(
            compiled.warnings,
            vec![CompileWarning::UnresolvedEntityRef(unknown_uuid)]
        );
    }

    #[test]
    fn in_hierarchy_invalid_right_operand() {
        let cond = Condition::InHierarchy(principal_var(), scalar_int(1));
        let err = compile(&cond).unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    #[test]
    fn in_hierarchy_context_scope_empty_path_is_error() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 1)]);
        let cond = Condition::InHierarchy(
            Operand::Variable(var(VariableScope::Context, &[])),
            Operand::EntityRef(entity_uuid),
        );
        let err = compile_with(&cond, &resolver).unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    #[test]
    fn in_hierarchy_invalid_left_non_variable() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 1)]);
        let cond = Condition::InHierarchy(scalar_int(1), Operand::EntityRef(entity_uuid));
        let err = compile_with(&cond, &resolver).unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    // ── 10. ContainsInHierarchy ──────────────────────────────────────────────

    #[test]
    fn contains_in_hierarchy_variable() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 7)]);
        let path_var = Operand::Variable(var(VariableScope::Principal, &[attr(3)]));
        let cond = Condition::ContainsInHierarchy(path_var, Operand::EntityRef(entity_uuid));
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushVariable(var(VariableScope::Principal, &[attr(3)])),
            OpCode::ContainsInHierarchy(7),
        ]);
    }

    #[test]
    fn contains_in_hierarchy_set_literal() {
        let entity_uuid = Uuid::new_v4();
        let uuid1 = Uuid::new_v4();
        let uuid2 = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 3)]);
        let set = Operand::Set(vec![
            Operand::EntityRef(uuid1),
            Operand::EntityRef(uuid2),
        ]);
        let cond = Condition::ContainsInHierarchy(set, Operand::EntityRef(entity_uuid));
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops.len(), 2);
        assert!(matches!(ops[0], OpCode::PushSet(_)));
        assert_eq!(ops[1], OpCode::ContainsInHierarchy(3));
    }

    #[test]
    fn contains_in_hierarchy_unresolved_uuid_compiles_to_false() {
        let unknown = Uuid::new_v4();
        let resolver = MockResolver::empty();
        let cond = Condition::ContainsInHierarchy(
            principal_var(),
            Operand::EntityRef(unknown),
        );
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![OpCode::PushBool(false)]);
    }

    #[test]
    fn contains_in_hierarchy_unresolved_uuid_produces_warning() {
        let unknown = Uuid::new_v4();
        let resolver = MockResolver::empty();
        let cond = Condition::ContainsInHierarchy(
            principal_var(),
            Operand::EntityRef(unknown),
        );
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert_eq!(
            compiled.warnings,
            vec![CompileWarning::UnresolvedEntityRef(unknown)]
        );
    }

    #[test]
    fn resolved_hierarchy_has_no_warnings() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 7)]);
        let cond = Condition::InHierarchy(
            principal_var(),
            Operand::EntityRef(entity_uuid),
        );
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert!(compiled.warnings.is_empty());
    }

    #[test]
    fn contains_in_hierarchy_invalid_scalar_left() {
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 1)]);
        let cond = Condition::ContainsInHierarchy(
            scalar_int(1),
            Operand::EntityRef(entity_uuid),
        );
        let err = compile_with(&cond, &resolver).unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    // ── 11. InNetwork ────────────────────────────────────────────────────────

    #[test]
    fn in_network_literal_ip_and_network() {
        use std::net::IpAddr;
        use ipnet::IpNet;
        let ip: IpAddr = "192.168.1.5".parse().unwrap();
        let net: IpNet = "192.168.0.0/16".parse().unwrap();
        let ops = compile(&Condition::InNetwork(Operand::IpAddr(ip), Operand::IpNetwork(net))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushIpAddr(ip),
            OpCode::PushIpNetwork(net),
            OpCode::InNetwork,
        ]);
    }

    #[test]
    fn in_network_variable_operands() {
        use ipnet::IpNet;
        let net: IpNet = "10.0.0.0/8".parse().unwrap();
        let ip_var = Operand::Variable(var(VariableScope::Context, &[attr(1)]));
        let ops = compile(&Condition::InNetwork(ip_var.clone(), Operand::IpNetwork(net))).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushVariable(var(VariableScope::Context, &[attr(1)])),
            OpCode::PushIpNetwork(net),
            OpCode::InNetwork,
        ]);
    }

    #[test]
    fn in_network_invalid_left_operand() {
        use ipnet::IpNet;
        let net: IpNet = "10.0.0.0/8".parse().unwrap();
        let err = compile(&Condition::InNetwork(scalar_str("not-an-ip"), Operand::IpNetwork(net)))
            .unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    #[test]
    fn in_network_invalid_right_operand() {
        use std::net::IpAddr;
        let ip: IpAddr = "10.1.2.3".parse().unwrap();
        let err = compile(&Condition::InNetwork(Operand::IpAddr(ip), scalar_str("not-a-net")))
            .unwrap_err();
        assert!(matches!(err, CompileError::InvalidOperand(_)));
    }

    // ── 12. Nested And/Or structure ──────────────────────────────────────────

    #[test]
    fn nested_and_or() {
        // (a == 1 || b == 2) && (c == 3)
        let a_eq_1 = Condition::Eq(scalar_int(1), scalar_int(1));
        let b_eq_2 = Condition::Eq(scalar_int(2), scalar_int(2));
        let c_eq_3 = Condition::Eq(scalar_int(3), scalar_int(3));
        let inner_or = Condition::Or(vec![a_eq_1, b_eq_2]);
        let outer_and = Condition::And(vec![inner_or, c_eq_3]);

        let ops = compile(&outer_and).unwrap();

        // Expected sequence with short-circuiting:
        // Or:
        //   PushScalar(1), PushScalar(1), Eq,
        //   JumpIfTrue(8)
        //   PushScalar(2), PushScalar(2), Eq,
        //   Jump(9)
        //   PushScalar(ScalarValue::Bool(true))
        // And:
        //   JumpIfFalse(14)
        //   PushScalar(3), PushScalar(3), Eq,
        //   Jump(15)
        //   PushScalar(ScalarValue::Bool(false))
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(1),
            OpCode::Eq,
            OpCode::JumpIfTrue(8),
            OpCode::PushInteger(2),
            OpCode::PushInteger(2),
            OpCode::Eq,
            OpCode::Jump(9),
            OpCode::PushBool(true),
            OpCode::JumpIfFalse(14),
            OpCode::PushInteger(3),
            OpCode::PushInteger(3),
            OpCode::Eq,
            OpCode::Jump(15),
            OpCode::PushBool(false),
        ]);
    }

    #[test]
    fn deeply_nested_not() {
        // NOT (NOT (x == 1))
        let inner = Condition::Eq(scalar_int(1), scalar_int(1));
        let double_not = Condition::Not(Box::new(Condition::Not(Box::new(inner))));
        let ops = compile(&double_not).unwrap();
        assert_eq!(ops, vec![
            OpCode::PushInteger(1),
            OpCode::PushInteger(1),
            OpCode::Eq,
            OpCode::Not,
            OpCode::Not,
        ]);
    }

    // ── 13. dependencies field ───────────────────────────────────────────────

    #[test]
    fn dependencies_single_variable() {
        let v = var(VariableScope::Principal, &[attr(1)]);
        let cond = Condition::Eq(Operand::Variable(v.clone()), scalar_int(0));
        let resolver = MockResolver::empty();
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert_eq!(compiled.dependencies, vec![v]);
    }

    #[test]
    fn dependencies_multiple_variables_deduped() {
        let v1 = var(VariableScope::Principal, &[attr(1)]);
        let v2 = var(VariableScope::Resource, &[attr(2)]);
        // v1 appears twice; v2 once.
        let cond = Condition::And(vec![
            Condition::Eq(Operand::Variable(v1.clone()), scalar_int(0)),
            Condition::Eq(Operand::Variable(v1.clone()), scalar_int(1)),
            Condition::Eq(Operand::Variable(v2.clone()), scalar_int(2)),
        ]);
        let resolver = MockResolver::empty();
        let compiled = compile_full(&cond, &resolver).unwrap();
        // Sorted + deduped.
        let mut expected = vec![v1, v2];
        expected.sort();
        assert_eq!(compiled.dependencies, expected);
    }

    #[test]
    fn dependencies_no_variables() {
        let cond = Condition::Eq(scalar_int(1), scalar_int(2));
        let resolver = MockResolver::empty();
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert!(compiled.dependencies.is_empty());
    }

    #[test]
    fn dependencies_from_is_type_empty() {
        // IsType has no operands — no variable dependencies.
        let cond = Condition::IsType(VariableScope::Principal, type_id(1));
        let resolver = MockResolver::empty();
        let compiled = compile_full(&cond, &resolver).unwrap();
        assert!(compiled.dependencies.is_empty());
    }

    // ── 14. Condition::Operand(Variable) ────────────────────────────────────

    #[test]
    fn bare_variable_operand() {
        let v = var(VariableScope::Principal, &[attr(7)]);
        let cond = Condition::Operand(Operand::Variable(v.clone()));
        let err = compile(&cond).unwrap_err();
        assert_eq!(err, CompileError::InvalidOperand("Bare operand in Condition must be Bool".into()));
    }

    #[test]
    fn bare_boolean_operand() {
        let cond = Condition::Operand(scalar_bool(true));
        let ops = compile(&cond).unwrap();
        assert_eq!(ops, vec![OpCode::PushBool(true)]);
    }

    #[test]
    fn bare_integer_operand() {
        let cond = Condition::Operand(scalar_int(99));
        let err = compile(&cond).unwrap_err();
        assert_eq!(err, CompileError::InvalidOperand("Bare operand in Condition must be Bool".into()));
    }

    #[test]
    fn bare_entity_ref_operand() {
        let uuid = Uuid::new_v4();
        let cond = Condition::Operand(Operand::EntityRef(uuid));
        let err = compile(&cond).unwrap_err();
        assert_eq!(err, CompileError::InvalidOperand("Bare operand in Condition must be Bool".into()));
    }

    // ── Additional edge cases ────────────────────────────────────────────────

    #[test]
    fn in_hierarchy_var_context_scope_with_path_is_allowed() {
        // Context scope with a non-empty path uses InHierarchyVar — this is valid
        // because InHierarchyVar resolves the entity from an attribute, not from
        // the context entity slot directly.
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 42)]);
        let ctx_var = Operand::Variable(var(VariableScope::Context, &[attr(1)]));
        let cond = Condition::InHierarchy(ctx_var, Operand::EntityRef(entity_uuid));
        let ops = compile_with(&cond, &resolver).unwrap();
        assert_eq!(ops, vec![
            OpCode::InHierarchyVar(var(VariableScope::Context, &[attr(1)]), 42),
        ]);
    }

    #[test]
    fn compile_full_instructions_and_dependencies() {
        // Full integration: And of two conditions with distinct variable refs.
        // v_principal has a path (attribute lookup), v_resource is root scope
        // (empty path) so InHierarchy is emitted instead of InHierarchyVar.
        let v_principal = var(VariableScope::Principal, &[attr(1)]);
        let v_resource_root = var(VariableScope::Resource, &[]);
        let entity_uuid = Uuid::new_v4();
        let resolver = MockResolver::new(vec![(entity_uuid, 0)]);

        let cond = Condition::And(vec![
            Condition::Eq(
                Operand::Variable(v_principal.clone()),
                scalar_str("alice"),
            ),
            Condition::InHierarchy(
                Operand::Variable(v_resource_root.clone()),
                Operand::EntityRef(entity_uuid),
            ),
        ]);

        let compiled = compile_full(&cond, &resolver).unwrap();

        // Instructions: PushVariable, PushScalar, Eq, JumpIfFalse(6), InHierarchy(bare), Jump(7), PushScalar(Bool(false))
        assert_eq!(compiled.instructions, vec![
            OpCode::PushVariable(v_principal.clone()),
            OpCode::PushString("alice".into()),
            OpCode::Eq,
            OpCode::JumpIfFalse(6),
            // Root-scope resource with empty path → InHierarchy (no PushVariable).
            OpCode::InHierarchy(VariableScope::Resource, 0),
            OpCode::Jump(7),
            OpCode::PushBool(false),
        ]);

        // v_principal is a dependency; v_resource_root has empty path and is
        // used only in InHierarchy (no stack push) — but compute_dependencies
        // still records it as a VariableRef dependency.
        let mut expected_deps = vec![v_principal, v_resource_root];
        expected_deps.sort();
        assert_eq!(compiled.dependencies, expected_deps);
    }
}
