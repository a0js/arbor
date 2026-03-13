use std::path::Path;
use std::sync::Arc;
use std::{fs, io};
use std::ops::Sub;
use arbor_bytecode::BytecodeVM;
use arbor_index_snapshot::{PackagedSnapshot, PolicySide, SerializationError, Snapshot};
use arbor_types::{ArborError, ConditionResult, EvaluationContext, EvaluationError, EntityTypeId, IndexedEntity};
use roaring::RoaringBitmap;

// ---------------------------------------------------------------------------
// StartupError
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum StartupError {
    #[error("failed to read snapshot file: {0}")]
    Io(#[from] io::Error),

    #[error("failed to deserialize snapshot: {0}")]
    Deserialization(#[from] SerializationError),
}

// ---------------------------------------------------------------------------
// AuthorizerError
// ---------------------------------------------------------------------------

#[derive(Debug, thiserror::Error)]
pub enum AuthorizerError {
    #[error("entity not found: index {0}")]
    EntityNotFound(u32),

    #[error("policy not found: index {0}")]
    PolicyNotFound(u32),

    #[error("condition eval error on policy {policy_idx}: {errors:?}")]
    ConditionEvaluation { policy_idx: u32, errors: Vec<EvaluationError> },

    #[error("snapshot error: {0}")]
    Snapshot(#[from] ArborError),
}

// ---------------------------------------------------------------------------
// Result types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub enum Decision {
    Permit,
    Deny,
}

pub struct CheckResult {
    pub decision: Decision,
    pub reason_policy_indices: Vec<u32>,
}

pub struct ListEntitiesResult {
    pub indices: Vec<u32>,
}

// ---------------------------------------------------------------------------
// AuthorizerEngine
// ---------------------------------------------------------------------------

pub struct AuthorizerEngine {
    pub(crate) snapshot: Arc<Snapshot>,
    pub(crate) version: u64,
}

impl AuthorizerEngine {
    pub fn load(path: &Path) -> Result<Self, StartupError> {
        let bytes = fs::read(path)?;
        let packaged = PackagedSnapshot::deserialize(&bytes)?;
        let version = packaged.version;
        let metadata = packaged.metadata.clone();
        let snapshot = packaged.into_snapshot()?;
        tracing::info!(
            version,
            entity_count = metadata.entity_count,
            policy_count = metadata.policy_count,
            action_count = metadata.action_count,
            "snapshot loaded"
        );
        Ok(Self { snapshot: Arc::new(snapshot), version })
    }

    pub fn check(
        &self,
        principal_idx: u32,
        action_idx: u32,
        resource_idx: u32,
    ) -> Result<CheckResult, AuthorizerError> {
        let principal_entity = self.snapshot.get_entity(principal_idx)
            .ok_or(AuthorizerError::EntityNotFound(principal_idx))?;
        let resource_entity = self.snapshot.get_entity(resource_idx)
            .ok_or(AuthorizerError::EntityNotFound(resource_idx))?;

        let action_policies = self.snapshot.get_policies_for_action(action_idx)
            .map_err(AuthorizerError::Snapshot)?;

        let principal_policies = principal_entity.effective_principal_policies.as_ref();
        let resource_policies = resource_entity.effective_resource_policies.as_ref();

        let effective_policies = match (principal_policies, resource_policies) {
            (Some(p), Some(r)) => {
                let mut eff = p & r;
                eff &= action_policies;
                eff
            }
            _ => RoaringBitmap::new(),
        };
        let (unconditional_forbidding, conditional_forbidding, unconditional_permitting, conditional_permitting) =
            self.snapshot.split_policy_map_for_authorization(&effective_policies);

        if !unconditional_forbidding.is_empty() {
            return Ok(CheckResult {
                decision: Decision::Deny,
                reason_policy_indices: unconditional_forbidding.iter().collect(),
            });
        }

        if conditional_forbidding.is_empty() && !unconditional_permitting.is_empty() {
            return Ok(CheckResult {
                decision: Decision::Permit,
                reason_policy_indices: unconditional_permitting.iter().collect(),
            });
        }

        let evaluation_context = EvaluationContext::new(
            principal_entity,
            resource_entity,
            None,
            self.snapshot.as_ref(),
        );
        let mut vm = BytecodeVM::new();

        if let Some(policy_idx) = self.find_matching_conditional_policy(&mut vm, &evaluation_context, &conditional_forbidding, true)? {
            return Ok(CheckResult {
                decision: Decision::Deny,
                reason_policy_indices: vec![policy_idx],
            });
        }

        if !unconditional_permitting.is_empty() {
            return Ok(CheckResult {
                decision: Decision::Permit,
                reason_policy_indices: unconditional_permitting.iter().collect(),
            });
        }

        if let Some(policy_idx) = self.find_matching_conditional_policy(&mut vm, &evaluation_context, &conditional_permitting, false)? {
            return Ok(CheckResult {
                decision: Decision::Permit,
                reason_policy_indices: vec![policy_idx],
            });
        }

        Ok(CheckResult {
            decision: Decision::Deny,
            reason_policy_indices: vec![],
        })
    }

    /// List entities on `candidate_side` that the fixed entity can access via `action_idx`.
    ///
    /// - `PolicySide::Resource`: `fixed_idx` is the principal; returns permitted resources.
    /// - `PolicySide::Principal`: `fixed_idx` is the resource; returns permitted principals.
    pub fn list_entities(
        &self,
        fixed_idx: u32,
        action_idx: u32,
        candidate_type: EntityTypeId,
        candidate_side: PolicySide,
    ) -> Result<ListEntitiesResult, AuthorizerError> {
        let fixed_entity = self.snapshot.get_entity(fixed_idx)
            .ok_or(AuthorizerError::EntityNotFound(fixed_idx))?;
        let fixed_policies = match candidate_side {
            PolicySide::Resource => fixed_entity.effective_principal_policies.as_ref(),
            PolicySide::Principal => fixed_entity.effective_resource_policies.as_ref(),
        };
        let action_policies = self.snapshot.get_policies_for_action(action_idx)
            .map_err(AuthorizerError::Snapshot)?;

        let effective_policies = if let Some(fp) = fixed_policies {
            fp & action_policies
        } else {
            RoaringBitmap::new()
        };
        let (unconditional_forbidding, conditional_forbidding, unconditional_permitting, conditional_permitting) =
            self.snapshot.split_policy_map_for_authorization(&effective_policies);

        let unconditional_forbidden_targets = self.snapshot
            .get_entities_of_type_for_policies(&unconditional_forbidding, candidate_type, candidate_side)
            .map_err(AuthorizerError::Snapshot)?;
        let unconditional_permitted_targets = self.snapshot
            .get_entities_of_type_for_policies(&unconditional_permitting, candidate_type, candidate_side)
            .map_err(AuthorizerError::Snapshot)?;
        let potential_forbidden_targets = self.snapshot
            .get_entities_of_type_for_policies(&conditional_forbidding, candidate_type, candidate_side)
            .map_err(AuthorizerError::Snapshot)?;
        let potential_permitted_targets = self.snapshot
            .get_entities_of_type_for_policies(&conditional_permitting, candidate_type, candidate_side)
            .map_err(AuthorizerError::Snapshot)?;

        let all_potential_permitted_targets = &unconditional_permitted_targets | &potential_permitted_targets;
        let filtered_potential_forbidden_targets =
            (&potential_forbidden_targets & &all_potential_permitted_targets).sub(&unconditional_forbidden_targets);

        let verified_forbidden_targets = self.verify_conditional_candidates(
            fixed_entity,
            filtered_potential_forbidden_targets,
            |idx| self.snapshot.get_effective_policies_intersected(idx, &conditional_forbidding, candidate_side).map_err(AuthorizerError::Snapshot),
            candidate_side,
            true,
        )?;

        let all_forbidden_targets = &unconditional_forbidden_targets | &verified_forbidden_targets;
        let filtered_potential_permitted_targets =
            potential_permitted_targets.sub(&unconditional_permitted_targets).sub(&all_forbidden_targets);

        let verified_permitted_targets = self.verify_conditional_candidates(
            fixed_entity,
            filtered_potential_permitted_targets,
            |idx| self.snapshot.get_effective_policies_intersected(idx, &conditional_permitting, candidate_side).map_err(AuthorizerError::Snapshot),
            candidate_side,
            false,
        )?;

        let permitted_targets = (&unconditional_permitted_targets | &verified_permitted_targets).sub(&all_forbidden_targets);

        Ok(ListEntitiesResult {
            indices: permitted_targets.into_iter().collect(),
        })
    }
}

impl AuthorizerEngine {
    fn find_matching_conditional_policy(
        &self,
        vm: &mut BytecodeVM,
        ctx: &EvaluationContext<'_>,
        policies: &RoaringBitmap,
        forbid_semantics: bool,
    ) -> Result<Option<u32>, AuthorizerError> {
        for policy_idx in policies.iter() {
            let policy = self.snapshot.get_policy(policy_idx)
                .ok_or(AuthorizerError::PolicyNotFound(policy_idx))?;
            if let Some(condition) = &policy.conditions {
                let is_match = match vm.evaluate(&condition.instructions, ctx) {
                    ConditionResult::True => true,
                    ConditionResult::Invalid(_) => forbid_semantics,
                    ConditionResult::False => false,
                };
                if is_match {
                    return Ok(Some(policy_idx));
                }
            }
        }
        Ok(None)
    }

    fn verify_conditional_candidates<'s>(
        &'s self,
        fixed_entity: &'s IndexedEntity,
        candidates: RoaringBitmap,
        get_candidate_policies: impl Fn(u32) -> Result<RoaringBitmap, AuthorizerError>,
        candidate_side: PolicySide,
        forbid_semantics: bool,
    ) -> Result<RoaringBitmap, AuthorizerError> {
        let mut verified = RoaringBitmap::new();
        let mut vm = BytecodeVM::new();
        for entity_idx in candidates {
            let entity = self.snapshot.get_entity(entity_idx)
                .ok_or(AuthorizerError::EntityNotFound(entity_idx))?;
            let entity_policies = get_candidate_policies(entity_idx)?;
            let (principal, resource) = match candidate_side {
                PolicySide::Resource => (fixed_entity, entity),
                PolicySide::Principal => (entity, fixed_entity),
            };
            let ctx = EvaluationContext::new(principal, resource, None, self.snapshot.as_ref());
            for policy_idx in entity_policies.iter() {
                let policy = self.snapshot.get_policy(policy_idx)
                    .ok_or(AuthorizerError::PolicyNotFound(policy_idx))?;
                if let Some(condition) = &policy.conditions {
                    let is_match = match vm.evaluate(&condition.instructions, &ctx) {
                        ConditionResult::True => true,
                        ConditionResult::Invalid(_) => forbid_semantics,
                        ConditionResult::False => false,
                    };
                    if is_match {
                        verified.insert(entity_idx);
                        break;
                    }
                }
            }
        }
        Ok(verified)
    }
}