//! Tenant administration endpoints.
//!
//! All endpoints require `Admin` role. Only available when multi-tenancy is enabled.

use std::sync::Arc;

use aivyx_audit::AuditEvent;
use aivyx_core::AivyxError;
use axum::extract::{Path, State};
use axum::response::IntoResponse;
use serde::{Deserialize, Serialize};

use aivyx_tenant::{AivyxRole, ResourceQuotas, TenantDirs, TenantRecord, TenantStatus};

use crate::app_state::AppState;
use crate::error::ServerError;
use crate::extractors::AuthContextExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

fn require_tenant_store(state: &AppState) -> Result<&aivyx_tenant::TenantStore, ServerError> {
    state
        .tenant_store
        .as_ref()
        .map(|s| s.as_ref())
        .ok_or_else(|| ServerError(AivyxError::Config("multi-tenancy not enabled".into())))
}

fn require_api_key_store(state: &AppState) -> Result<&aivyx_tenant::ApiKeyStore, ServerError> {
    state
        .api_key_store
        .as_ref()
        .map(|s| s.as_ref())
        .ok_or_else(|| ServerError(AivyxError::Config("multi-tenancy not enabled".into())))
}

fn parse_tenant_id(id: &str) -> Result<aivyx_core::TenantId, ServerError> {
    id.parse()
        .map_err(|_| ServerError(AivyxError::Config(format!("invalid tenant ID: {id}"))))
}

// ---------------------------------------------------------------------------
// POST /tenants — create tenant
// ---------------------------------------------------------------------------

/// Request body for `POST /tenants`.
#[derive(Debug, Deserialize)]
pub struct CreateTenantRequest {
    pub name: String,
    #[serde(default)]
    pub quotas: Option<ResourceQuotas>,
}

/// `POST /tenants` — create a new tenant.
pub async fn create_tenant(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    axum::Json(body): axum::Json<CreateTenantRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;

    let quotas = body.quotas.unwrap_or_default();
    let record = store.create_tenant(&body.name, quotas, &state.master_key)?;

    // Create per-tenant directory structure
    let tenant_dirs = TenantDirs::new(state.dirs.root(), &record.id);
    tenant_dirs.ensure_dirs()?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::TenantCreated {
        tenant_id: record.id.to_string(),
        name: record.name.clone(),
    }) {
        tracing::warn!("failed to audit tenant creation: {e}");
    }

    Ok((axum::http::StatusCode::CREATED, axum::Json(record)))
}

// ---------------------------------------------------------------------------
// GET /tenants — list tenants
// ---------------------------------------------------------------------------

/// `GET /tenants` — list all non-deleted tenants.
pub async fn list_tenants(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;

    let tenants = store.list_tenants(&state.master_key)?;
    Ok(axum::Json(tenants))
}

// ---------------------------------------------------------------------------
// GET /tenants/{id} — get tenant
// ---------------------------------------------------------------------------

/// `GET /tenants/{id}` — get a single tenant by ID.
pub async fn get_tenant(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    let record = store
        .get_tenant(&tenant_id, &state.master_key)?
        .ok_or_else(|| {
            ServerError(AivyxError::Config(format!("tenant not found: {id}")))
        })?;

    Ok(axum::Json(record))
}

// ---------------------------------------------------------------------------
// POST /tenants/{id}/suspend
// ---------------------------------------------------------------------------

/// Request body for `POST /tenants/{id}/suspend`.
#[derive(Debug, Deserialize)]
pub struct SuspendRequest {
    pub reason: String,
}

/// `POST /tenants/{id}/suspend` — suspend a tenant.
pub async fn suspend_tenant(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<SuspendRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    store.update_status(
        &tenant_id,
        TenantStatus::Suspended {
            reason: body.reason.clone(),
        },
        &state.master_key,
    )?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::TenantSuspended {
        tenant_id: tenant_id.to_string(),
        reason: body.reason,
    }) {
        tracing::warn!("failed to audit tenant suspension: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /tenants/{id}/unsuspend
// ---------------------------------------------------------------------------

/// `POST /tenants/{id}/unsuspend` — reactivate a suspended tenant.
pub async fn unsuspend_tenant(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    store.update_status(&tenant_id, TenantStatus::Active, &state.master_key)?;

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// DELETE /tenants/{id} — soft delete
// ---------------------------------------------------------------------------

/// `DELETE /tenants/{id}` — soft-delete a tenant.
pub async fn delete_tenant(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let store = require_tenant_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    store.delete_tenant(&tenant_id, &state.master_key)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::TenantDeleted {
        tenant_id: tenant_id.to_string(),
    }) {
        tracing::warn!("failed to audit tenant deletion: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /tenants/{id}/keys — create API key
// ---------------------------------------------------------------------------

/// Request body for `POST /tenants/{id}/keys`.
#[derive(Debug, Deserialize)]
pub struct CreateApiKeyRequest {
    pub name: String,
    pub role: AivyxRole,
    #[serde(default)]
    pub scopes: Vec<aivyx_tenant::ApiKeyScope>,
}

/// Response for `POST /tenants/{id}/keys` — returned only once.
#[derive(Debug, Serialize)]
pub struct CreateApiKeyResponse {
    /// The plaintext API token (shown only once).
    pub token: String,
    /// Key metadata.
    pub key: aivyx_tenant::ApiKeyRecord,
}

/// `POST /tenants/{id}/keys` — create a new API key for a tenant.
pub async fn create_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
    axum::Json(body): axum::Json<CreateApiKeyRequest>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let key_store = require_api_key_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    let scope_strings: Vec<String> = body.scopes.iter().map(|s| format!("{s:?}")).collect();

    let (token, record) = key_store.create_key(
        tenant_id,
        &body.name,
        body.role,
        body.scopes,
        None,
        &state.master_key,
    )?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::ApiKeyCreated {
        tenant_id: tenant_id.to_string(),
        key_id: record.key_id.clone(),
        scopes: scope_strings,
    }) {
        tracing::warn!("failed to audit API key creation: {e}");
    }

    Ok((
        axum::http::StatusCode::CREATED,
        axum::Json(CreateApiKeyResponse { token, key: record }),
    ))
}

// ---------------------------------------------------------------------------
// GET /tenants/{id}/keys — list keys
// ---------------------------------------------------------------------------

/// `GET /tenants/{id}/keys` — list API key metadata for a tenant.
pub async fn list_api_keys(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path(id): Path<String>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let key_store = require_api_key_store(&state)?;
    let tenant_id = parse_tenant_id(&id)?;

    let keys = key_store.list_keys(&tenant_id, &state.master_key)?;
    Ok(axum::Json(keys))
}

// ---------------------------------------------------------------------------
// DELETE /tenants/{id}/keys/{key_id} — revoke key
// ---------------------------------------------------------------------------

/// `DELETE /tenants/{id}/keys/{key_id}` — revoke an API key.
pub async fn revoke_api_key(
    State(state): State<Arc<AppState>>,
    auth: AuthContextExt,
    Path((id, key_id)): Path<(String, String)>,
) -> Result<impl IntoResponse, ServerError> {
    auth.require_role(AivyxRole::Admin)?;
    let key_store = require_api_key_store(&state)?;
    let _tenant_id = parse_tenant_id(&id)?;

    key_store.revoke_key(&key_id, &state.master_key)?;

    // Audit
    if let Err(e) = state.audit_log.append(AuditEvent::ApiKeyRevoked {
        tenant_id: id,
        key_id,
    }) {
        tracing::warn!("failed to audit API key revocation: {e}");
    }

    Ok(axum::http::StatusCode::NO_CONTENT)
}
