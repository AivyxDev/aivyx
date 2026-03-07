# Multi-Tenancy with OIDC Single Sign-On

This guide covers setting up multi-tenant isolation in Aivyx Engine with
OpenID Connect (OIDC) for single sign-on, role-based access control, and
per-tenant cost allocation.

## Step 1: Enable Multi-Tenancy

Add the tenants section to `aivyx.toml`:

```toml
[tenants]
enabled = true
```

Restart Aivyx Engine to enable the multi-tenancy subsystem. When enabled,
all API requests are scoped to a tenant, and per-tenant encryption keys are
derived automatically (see ADR-0001).

## Step 2: Create Tenants

Create tenants via the admin API:

```bash
curl -s -X POST http://localhost:3000/tenants \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Acme Corp",
    "slug": "acme-corp",
    "contact_email": "admin@acme.example.com"
  }' | jq .
```

Response:

```json
{
  "id": "tn_01HXYZ...",
  "name": "Acme Corp",
  "slug": "acme-corp",
  "contact_email": "admin@acme.example.com",
  "status": "active",
  "created_at": "2026-03-07T12:00:00Z"
}
```

## Step 3: Configure Resource Quotas

Set per-tenant limits to prevent any single tenant from monopolizing resources:

```bash
curl -s -X PATCH http://localhost:3000/tenants/tn_01HXYZ.../quotas \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "max_agents": 20,
    "max_sessions_per_hour": 100,
    "daily_budget_usd": 50.00,
    "monthly_budget_usd": 1000.00,
    "max_memory_entries": 100000,
    "max_storage_bytes": 1073741824
  }' | jq .
```

Quotas are enforced in real time. When a quota is exceeded, the API returns
`429 Too Many Requests` with a descriptive error message.

## Step 4: Generate Tenant API Keys

Each tenant needs API keys for their users and integrations:

```bash
curl -s -X POST http://localhost:3000/tenants/tn_01HXYZ.../keys \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "Production API Key",
    "role": "Operator",
    "expires_at": "2027-03-07T00:00:00Z"
  }' | jq .
```

Response:

```json
{
  "id": "key_01HABC...",
  "name": "Production API Key",
  "key": "aivyx_tn_01HXYZ..._sk_...",
  "role": "Operator",
  "expires_at": "2027-03-07T00:00:00Z"
}
```

The `key` value is shown only once at creation time. Store it securely.

Tenant API keys are prefixed with the tenant ID, so the engine automatically
scopes all requests to the correct tenant.

## Step 5: Configure OIDC Single Sign-On

Add OIDC configuration to `aivyx.toml`:

```toml
[sso]
enabled = true

[sso.oidc]
issuer_url = "https://auth.example.com"
client_id = "aivyx"
client_secret_ref = "OIDC_CLIENT_SECRET"
redirect_uri = "https://aivyx.example.com/auth/callback"
scopes = ["openid", "profile", "email", "groups"]

[[sso.group_role_mappings]]
group = "aivyx-admins"
role = "Admin"

[[sso.group_role_mappings]]
group = "aivyx-operators"
role = "Operator"

[[sso.group_role_mappings]]
group = "aivyx-viewers"
role = "Viewer"

[[sso.group_role_mappings]]
group = "aivyx-billing"
role = "Billing"
```

Set the OIDC client secret:

```bash
export OIDC_CLIENT_SECRET="your-oidc-client-secret"
```

### OIDC provider setup

Configure your identity provider (Okta, Auth0, Keycloak, Azure AD, etc.)
with:

| Field          | Value                                       |
|----------------|---------------------------------------------|
| Redirect URI   | `https://aivyx.example.com/auth/callback`   |
| Allowed scopes | `openid`, `profile`, `email`, `groups`      |
| Grant type     | Authorization Code with PKCE                |
| Token format   | JWT                                         |

Ensure the `groups` claim is included in the ID token. In most providers,
this requires enabling a "groups" scope or configuring a custom claim.

### Tenant-to-OIDC mapping

Map OIDC organizations or groups to tenants:

```toml
[[sso.tenant_mappings]]
oidc_org = "acme-corp"
tenant_slug = "acme-corp"

[[sso.tenant_mappings]]
oidc_org = "globex-inc"
tenant_slug = "globex-inc"
```

When a user authenticates, Aivyx resolves their tenant from the OIDC
organization claim and their role from the group mappings.

## Step 6: RBAC Role Hierarchy

Aivyx defines four roles in a strict hierarchy:

```
Billing < Viewer < Operator < Admin
```

| Role     | Permissions                                              |
|----------|----------------------------------------------------------|
| Billing  | View usage and cost data only                            |
| Viewer   | Read-only access to agents, sessions, and memory         |
| Operator | Full operational access (chat, create agents, manage memory) |
| Admin    | Everything, plus tenant management, user management, and configuration |

Each role inherits all permissions of the roles below it. For example, an
Admin can do everything an Operator, Viewer, and Billing user can do.

### API permission enforcement

Every API endpoint checks the caller's role:

```
POST /chat           --> requires Operator
GET  /agents         --> requires Viewer
POST /agents         --> requires Operator
GET  /usage          --> requires Billing
POST /tenants        --> requires Admin
POST /tenants/{id}/keys --> requires Admin
```

Insufficient permissions return `403 Forbidden`:

```json
{
  "error": "forbidden",
  "message": "This action requires the Admin role"
}
```

## Step 7: Cost Allocation Tags

Use the `X-Aivyx-Tags` header to tag API requests for cost attribution:

```bash
curl -s -X POST http://localhost:3000/chat \
  -H "Authorization: Bearer $TENANT_API_KEY" \
  -H "Content-Type: application/json" \
  -H "X-Aivyx-Tags: project:atlas,team:backend,env:production" \
  -d '{
    "agent": "default",
    "message": "Analyze this pull request."
  }' | jq .
```

Tags flow through to the cost ledger (see ADR-0005) and can be queried via
the usage API:

```bash
# Get usage for a specific project
curl -s "http://localhost:3000/usage?tags=project:atlas" \
  -H "Authorization: Bearer $TENANT_API_KEY" | jq .

# Get daily breakdown by team
curl -s "http://localhost:3000/usage/daily?group_by=tag:team" \
  -H "Authorization: Bearer $TENANT_API_KEY" | jq .
```

## Step 8: Tenant Lifecycle Management

### Suspend a tenant

Suspended tenants cannot make API requests. Existing data is preserved.

```bash
curl -s -X POST http://localhost:3000/tenants/tn_01HXYZ.../suspend \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "reason": "Non-payment"
  }' | jq .
```

Suspended tenant API keys return `403 Forbidden`:

```json
{
  "error": "tenant_suspended",
  "message": "Tenant is suspended: Non-payment"
}
```

### Unsuspend a tenant

```bash
curl -s -X POST http://localhost:3000/tenants/tn_01HXYZ.../unsuspend \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" | jq .
```

### List all tenants

```bash
curl -s http://localhost:3000/tenants \
  -H "Authorization: Bearer $AIVYX_ADMIN_TOKEN" | jq .
```

## Data Isolation

Multi-tenancy in Aivyx provides strong isolation guarantees:

- **Encryption isolation**: each tenant's data is encrypted with a distinct
  key derived via `derive_tenant_key(master, tenant_id)` (see ADR-0001).
  Compromise of one tenant's data does not expose another's.
- **Query isolation**: all database queries are scoped by `tenant_id`. There
  is no API operation that returns data across tenants (except admin endpoints).
- **Capability isolation**: each tenant's agents have independent capability
  sets. A tenant cannot configure agents with capabilities beyond their
  quota allows.
- **Budget isolation**: per-tenant budgets are enforced independently. One
  tenant's spending does not affect another's budget.
