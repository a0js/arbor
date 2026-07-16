use std::path::Path;
use std::str::FromStr;
use std::sync::Arc;
use arbor_proto_internal::arbor_v1::{
    self,
    arbor_server::Arbor,
    CheckBatchRequest, CheckBatchResponse, CheckResultItem,
    CheckRequest, CheckResponse,
    ListActionsRequest, ListActionsResponse,
    ListPrincipalsRequest, ListPrincipalsResponse,
    ListResourcesRequest, ListResourcesResponse,
};
use tonic::{Request, Response, Status};
use uuid::Uuid;
use arbor_proto_internal::arbor_v1::check_response::Decision as ProtoDecision;
use arbor_index_snapshot::PolicySide;
use crate::engine::{AuthorizerEngine, AuthorizerError, Decision, StartupError};

// Allow main.rs to wrap the service in the tonic-generated server type.
pub use arbor_v1::arbor_server::ArborServer;

// ---------------------------------------------------------------------------
// AuthorizerService
// ---------------------------------------------------------------------------

pub struct AuthorizerService {
    engine: Arc<AuthorizerEngine>,
}

impl AuthorizerService {
    /// Loads a snapshot written by `IndexerService::rebuild_snapshot`
    /// (the rkyv+lz4 pipeline).
    pub fn load(path: &Path) -> Result<Self, StartupError> {
        Ok(Self { engine: Arc::new(AuthorizerEngine::load_rkyv(path)?) })
    }

    /// Read-only access to the underlying engine, e.g. for diagnostics or
    /// embedding this service outside its own gRPC handlers.
    pub fn engine(&self) -> &AuthorizerEngine {
        &self.engine
    }

    fn resolve_uuid(&self, s: &str, field: &'static str) -> Result<u32, Status> {
        let uuid = Uuid::from_str(s)
            .map_err(|_| Status::invalid_argument(format!("invalid {field}: not a UUID")))?;
        self.engine.snapshot().uuid_to_index(&uuid)
            .ok_or_else(|| Status::not_found(format!("{field} not found")))
    }

    fn indices_to_uuid_strings(&self, indices: &[u32]) -> Vec<String> {
        indices.iter()
            .filter_map(|&i| {
                let uuid = self.engine.snapshot().index_to_uuid(i);
                if uuid.is_none() {
                    tracing::warn!(index = i, "index has no UUID — snapshot may be corrupt or index out of range");
                }
                uuid
            })
            .map(|u| u.to_string())
            .collect()
    }
}

fn engine_error_to_status(e: AuthorizerError) -> Status {
    match e {
        AuthorizerError::EntityNotFound(idx) =>
            Status::internal(format!("entity {idx} indexed but missing from snapshot")),
        AuthorizerError::PolicyNotFound(idx) =>
            Status::internal(format!("policy {idx} indexed but missing from snapshot")),
        AuthorizerError::ConditionEvaluation { policy_idx, errors } =>
            Status::internal(format!("condition eval error on policy {policy_idx}: {errors:?}")),
        AuthorizerError::Snapshot(e) =>
            Status::internal(format!("snapshot error: {e}")),
    }
}

// ---------------------------------------------------------------------------
// gRPC handlers
// ---------------------------------------------------------------------------

#[tonic::async_trait]
impl Arbor for AuthorizerService {
    async fn check(
        &self,
        request: Request<CheckRequest>,
    ) -> Result<Response<CheckResponse>, Status> {
        let req = request.into_inner();
        let principal_idx = self.resolve_uuid(&req.principal_id, "principal_id")?;
        let action_idx    = self.resolve_uuid(&req.action_id, "action_id")?;
        let resource_idx  = self.resolve_uuid(&req.resource_id, "resource_id")?;

        let result = self.engine.check(principal_idx, action_idx, resource_idx)
            .map_err(engine_error_to_status)?;

        let proto_decision = match result.decision {
            Decision::Permit => ProtoDecision::Permit,
            Decision::Deny   => ProtoDecision::Deny,
        };

        Ok(Response::new(CheckResponse {
            decision: proto_decision.into(),
            snapshot_version: self.engine.version,
            reasons: self.indices_to_uuid_strings(&result.reason_policy_indices),
        }))
    }

    async fn check_batch(
        &self,
        request: Request<CheckBatchRequest>,
    ) -> Result<Response<CheckBatchResponse>, Status> {
        let req = request.into_inner();
        let mut results = Vec::with_capacity(req.items.len());

        for item in &req.items {
            let (principal_idx, action_idx, resource_idx) = match (
                self.resolve_uuid(&item.principal_id, "principal_id"),
                self.resolve_uuid(&item.action_id, "action_id"),
                self.resolve_uuid(&item.resource_id, "resource_id"),
            ) {
                (Ok(p), Ok(a), Ok(r)) => (p, a, r),
                _ => {
                    results.push(CheckResultItem {
                        decision: ProtoDecision::Deny.into(),
                    });
                    continue;
                }
            };

            let result = self.engine.check(principal_idx, action_idx, resource_idx)
                .map_err(engine_error_to_status)?;

            let proto_decision = match result.decision {
                Decision::Permit => ProtoDecision::Permit,
                Decision::Deny   => ProtoDecision::Deny,
            };

            results.push(CheckResultItem {
                decision: proto_decision.into(),
            });
        }

        Ok(Response::new(CheckBatchResponse {
            results,
            snapshot_version: self.engine.version,
        }))
    }

    async fn list_resources(
        &self,
        request: Request<ListResourcesRequest>,
    ) -> Result<Response<ListResourcesResponse>, Status> {
        let req = request.into_inner();
        let principal_idx = self.resolve_uuid(&req.principal_id, "principal_id")?;
        let action_idx    = self.resolve_uuid(&req.action_id, "action_id")?;
        let resource_type = self.engine.snapshot().get_entity_type_id_by_name(&req.entity_type_id)
            .ok_or_else(|| Status::invalid_argument(
                format!("unknown entity_type_id: {:?}", req.entity_type_id)
            ))?;

        let result = self.engine.list_entities(principal_idx, action_idx, resource_type, PolicySide::Resource)
            .map_err(engine_error_to_status)?;

        Ok(Response::new(ListResourcesResponse {
            resource_ids: self.indices_to_uuid_strings(&result.indices),
            snapshot_version: self.engine.version,
            next_page_token: String::new(),
        }))
    }

    async fn list_principals(
        &self,
        request: Request<ListPrincipalsRequest>,
    ) -> Result<Response<ListPrincipalsResponse>, Status> {
        let req = request.into_inner();
        let resource_idx = self.resolve_uuid(&req.resource_id, "principal_id")?;
        let action_idx = self.resolve_uuid(&req.action_id, "action_id")?;
        let principal_type = self.engine.snapshot().get_entity_type_id_by_name(&req.entity_type_id)
            .ok_or_else(|| Status::invalid_argument(
                format!("unknown entity_type_id: {:?}", req.entity_type_id)
            ))?;

        let result = self.engine.list_entities(resource_idx, action_idx, principal_type, PolicySide::Principal)
            .map_err(engine_error_to_status)?;

        Ok(Response::new(ListPrincipalsResponse {
            principal_ids: self.indices_to_uuid_strings(&result.indices),
            snapshot_version: self.engine.version,
            next_page_token: String::new(),
        }))
    }

    async fn list_actions(
        &self,
        _request: Request<ListActionsRequest>,
    ) -> Result<Response<ListActionsResponse>, Status> {
        Err(Status::unimplemented("list_actions not yet implemented"))
    }
}
