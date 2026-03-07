# REST API Client Guide

This guide covers using the Aivyx Engine REST API for programmatic access
to agents, chat, memory, tasks, and team sessions.

## Authentication

All API requests require a bearer token in the `Authorization` header:

```
Authorization: Bearer your-aivyx-token
```

For multi-tenant deployments, use the tenant-scoped API key (see the
multi-tenant OIDC guide for key generation).

## Base URL

The default base URL is `http://localhost:3000`. Configure this based on
your deployment:

| Environment   | Base URL                          |
|---------------|-----------------------------------|
| Local dev     | `http://localhost:3000`            |
| Docker        | `http://aivyx-engine:3000`        |
| Kubernetes    | `https://aivyx.example.com`       |

## Error Handling

All error responses return JSON with `error` and `message` fields:

```json
{
  "error": "not_found",
  "message": "Agent 'nonexistent' not found"
}
```

Common HTTP status codes:

| Code | Meaning               | Action                            |
|------|-----------------------|-----------------------------------|
| 400  | Bad Request           | Fix the request body or parameters |
| 401  | Unauthorized          | Check your bearer token            |
| 403  | Forbidden             | Insufficient role permissions      |
| 404  | Not Found             | Check the resource ID or name      |
| 429  | Too Many Requests     | Wait and retry (see Retry-After)   |
| 500  | Internal Server Error | Report as a bug                    |

## Chat

### Send a message

```
POST /chat
```

```json
{
  "agent": "default",
  "message": "Explain the Rust ownership model in simple terms.",
  "session_id": "optional-session-id",
  "images": [
    {
      "media_type": "image/png",
      "data": "base64-encoded-image-data"
    }
  ]
}
```

Response:

```json
{
  "response": "Rust's ownership model is built on three rules...",
  "session_id": "ses_01HABC...",
  "usage": {
    "input_tokens": 42,
    "output_tokens": 256,
    "cost_usd": 0.0031
  }
}
```

The `session_id` can be reused in subsequent requests to maintain
conversation context.

### Stream a response

```
POST /chat/stream
```

Same request body as `/chat`. Returns a Server-Sent Events stream:

```
data: {"type":"text","content":"Rust's ownership"}
data: {"type":"text","content":" model is built on"}
data: {"type":"tool_call","name":"search_docs","input":{"query":"ownership"}}
data: {"type":"tool_result","name":"search_docs","output":"..."}
data: {"type":"text","content":" three rules..."}
data: {"type":"done","usage":{"input_tokens":42,"output_tokens":256}}
```

## Agents

### List agents

```
GET /agents
```

### Create an agent

```
POST /agents
```

```json
{
  "name": "code-reviewer",
  "system_prompt": "You are an expert code reviewer. Focus on correctness, performance, and idiomatic style.",
  "model": "claude-sonnet-4-20250514",
  "capabilities": {
    "shell": ["git diff *", "git log *"],
    "filesystem": ["/home/user/project/**"],
    "network": []
  },
  "plugins": ["github-tools"],
  "temperature": 0.3
}
```

### Get an agent

```
GET /agents/{name}
```

### Update an agent

```
PATCH /agents/{name}
```

### Delete an agent

```
DELETE /agents/{name}
```

## Memory

### Search memory

```
POST /memory/search
```

```json
{
  "query": "deployment procedures for the Atlas project",
  "limit": 10,
  "min_score": 0.7
}
```

Response:

```json
{
  "results": [
    {
      "id": "mem_01H...",
      "content": "Atlas deployment uses a blue-green strategy...",
      "score": 0.92,
      "metadata": {
        "source": "conversation",
        "agent": "default",
        "timestamp": "2026-03-06T14:30:00Z"
      }
    }
  ]
}
```

### Store a memory

```
POST /memory/store
```

```json
{
  "content": "The production database is PostgreSQL 16 on RDS.",
  "metadata": {
    "source": "manual",
    "tags": ["infrastructure", "database"]
  }
}
```

### Query the knowledge graph

```
POST /memory/graph
```

```json
{
  "query": "What tools does the Atlas project use?",
  "depth": 2
}
```

Response:

```json
{
  "nodes": [
    { "id": "n1", "label": "Atlas Project", "type": "project" },
    { "id": "n2", "label": "PostgreSQL", "type": "technology" },
    { "id": "n3", "label": "Kubernetes", "type": "technology" }
  ],
  "edges": [
    { "from": "n1", "to": "n2", "relation": "uses" },
    { "from": "n1", "to": "n3", "relation": "deployed_on" }
  ]
}
```

## Tasks

### Create a task

```
POST /tasks
```

```json
{
  "agent": "research-agent",
  "prompt": "Research the latest developments in WebAssembly and write a summary.",
  "priority": "normal"
}
```

### Get task status

```
GET /tasks/{id}
```

### Cancel a task

```
POST /tasks/{id}/cancel
```

## Teams

### Run a team session

```
POST /teams/run
```

```json
{
  "lead": "project-manager",
  "specialists": ["coder", "reviewer", "documenter"],
  "mission": "Implement a REST API endpoint for user registration with input validation, tests, and documentation.",
  "plan_type": "sequential"
}
```

Response:

```json
{
  "session_id": "team_01H...",
  "status": "completed",
  "steps": [
    { "agent": "project-manager", "kind": "Execute", "status": "completed" },
    { "agent": "coder", "kind": "Delegate", "status": "completed" },
    { "agent": "reviewer", "kind": "Reflect", "status": "completed" },
    { "agent": "documenter", "kind": "Delegate", "status": "completed" }
  ],
  "result": "All steps completed successfully.",
  "usage": {
    "total_cost_usd": 0.45,
    "by_agent": {
      "project-manager": 0.08,
      "coder": 0.25,
      "reviewer": 0.07,
      "documenter": 0.05
    }
  }
}
```

## Rate Limiting

When you exceed rate limits, the API returns `429 Too Many Requests` with a
`Retry-After` header:

```
HTTP/1.1 429 Too Many Requests
Retry-After: 30
Content-Type: application/json

{
  "error": "rate_limit_exceeded",
  "message": "Daily budget exceeded for agent 'research-agent'",
  "retry_after_seconds": 30
}
```

Always respect the `Retry-After` value before retrying.

## Usage and Billing

### Get aggregated usage

```
GET /usage?agent=research-agent&start=2026-03-01&end=2026-03-07
```

### Get daily breakdown

```
GET /usage/daily?group_by=agent
```

## Example: Python Client

```python
import requests

BASE_URL = "http://localhost:3000"
TOKEN = "your-aivyx-token"

headers = {
    "Authorization": f"Bearer {TOKEN}",
    "Content-Type": "application/json",
}

# Send a chat message
response = requests.post(
    f"{BASE_URL}/chat",
    headers=headers,
    json={
        "agent": "default",
        "message": "What is the capital of France?",
    },
)
data = response.json()
print(data["response"])

# Stream a response
response = requests.post(
    f"{BASE_URL}/chat/stream",
    headers=headers,
    json={"agent": "default", "message": "Write a poem about Rust."},
    stream=True,
)
for line in response.iter_lines():
    if line:
        line = line.decode("utf-8")
        if line.startswith("data: "):
            print(line[6:])
```

## Example: TypeScript Client

```typescript
const BASE_URL = "http://localhost:3000";
const TOKEN = "your-aivyx-token";

async function chat(message: string): Promise<string> {
  const response = await fetch(`${BASE_URL}/chat`, {
    method: "POST",
    headers: {
      Authorization: `Bearer ${TOKEN}`,
      "Content-Type": "application/json",
    },
    body: JSON.stringify({ agent: "default", message }),
  });

  if (!response.ok) {
    const error = await response.json();
    throw new Error(`${error.error}: ${error.message}`);
  }

  const data = await response.json();
  return data.response;
}

const answer = await chat("What is the capital of France?");
console.log(answer);
```

## Example: curl

```bash
# Chat
curl -s -X POST http://localhost:3000/chat \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"agent": "default", "message": "Hello!"}' | jq .

# List agents
curl -s http://localhost:3000/agents \
  -H "Authorization: Bearer $AIVYX_TOKEN" | jq .

# Search memory
curl -s -X POST http://localhost:3000/memory/search \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{"query": "deployment procedures", "limit": 5}' | jq .
```
