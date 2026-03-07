# Developing Custom MCP Servers for Aivyx

This guide covers building custom Model Context Protocol (MCP) servers and
integrating them with Aivyx Engine. MCP is the agent-to-tool protocol that
allows agents to discover and invoke external tools.

## MCP Overview

MCP defines a standard interface between AI agents (clients) and tool providers
(servers). Key characteristics:

- **Transport**: stdio (local processes) or SSE over HTTP (remote servers).
- **Discovery**: servers declare their available tools, each with a JSON Schema
  describing its input parameters.
- **Invocation**: clients call tools by name with validated arguments.
- **Bidirectional**: servers can request LLM completions from the client via
  `sampling/createMessage` (useful for tool chains that need AI reasoning).

## Step 1: Define Your Tool Schema

Each MCP tool has a name, description, and JSON Schema for its input:

```json
{
  "name": "lookup_weather",
  "description": "Get current weather for a location",
  "inputSchema": {
    "type": "object",
    "properties": {
      "location": {
        "type": "string",
        "description": "City name or coordinates (e.g., 'London' or '51.5,-0.1')"
      },
      "units": {
        "type": "string",
        "enum": ["celsius", "fahrenheit"],
        "default": "celsius"
      }
    },
    "required": ["location"]
  }
}
```

## Step 2: Implement a Stdio MCP Server (Node.js)

A stdio MCP server communicates over stdin/stdout using JSON-RPC 2.0 messages.

```javascript
// weather-mcp-server.js
const readline = require('readline');

const TOOLS = [
  {
    name: 'lookup_weather',
    description: 'Get current weather for a location',
    inputSchema: {
      type: 'object',
      properties: {
        location: { type: 'string', description: 'City name or coordinates' },
        units: { type: 'string', enum: ['celsius', 'fahrenheit'], default: 'celsius' },
      },
      required: ['location'],
    },
  },
];

async function handleRequest(request) {
  const { method, params, id } = request;

  switch (method) {
    case 'initialize':
      return {
        jsonrpc: '2.0',
        id,
        result: {
          protocolVersion: '2024-11-05',
          capabilities: { tools: {} },
          serverInfo: { name: 'weather-server', version: '1.0.0' },
        },
      };

    case 'tools/list':
      return {
        jsonrpc: '2.0',
        id,
        result: { tools: TOOLS },
      };

    case 'tools/call':
      if (params.name === 'lookup_weather') {
        const { location, units = 'celsius' } = params.arguments;
        // Replace with actual weather API call
        const weather = await fetchWeather(location, units);
        return {
          jsonrpc: '2.0',
          id,
          result: {
            content: [{ type: 'text', text: JSON.stringify(weather) }],
          },
        };
      }
      return {
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `Unknown tool: ${params.name}` },
      };

    default:
      return {
        jsonrpc: '2.0',
        id,
        error: { code: -32601, message: `Unknown method: ${method}` },
      };
  }
}

async function fetchWeather(location, units) {
  // Placeholder -- integrate your preferred weather API here
  return {
    location,
    temperature: units === 'celsius' ? 18 : 64,
    units,
    condition: 'partly cloudy',
    humidity: 65,
  };
}

const rl = readline.createInterface({ input: process.stdin });
rl.on('line', async (line) => {
  try {
    const request = JSON.parse(line);
    const response = await handleRequest(request);
    process.stdout.write(JSON.stringify(response) + '\n');
  } catch (err) {
    const errorResponse = {
      jsonrpc: '2.0',
      id: null,
      error: { code: -32700, message: 'Parse error' },
    };
    process.stdout.write(JSON.stringify(errorResponse) + '\n');
  }
});
```

## Step 3: Register the MCP Server in Aivyx

### Stdio server (local)

Register a local stdio MCP server via the plugin API:

```bash
curl -s -X POST http://localhost:3000/plugins \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "weather-tools",
    "type": "mcp",
    "config": {
      "transport": "stdio",
      "command": "node",
      "args": ["/path/to/weather-mcp-server.js"]
    }
  }' | jq .
```

Or configure it in `aivyx.toml`:

```toml
[[plugins]]
name = "weather-tools"
type = "mcp"

[plugins.config]
transport = "stdio"
command = "node"
args = ["/path/to/weather-mcp-server.js"]
```

### SSE server (remote)

For remote MCP servers accessible over HTTP:

```bash
curl -s -X POST http://localhost:3000/plugins \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "remote-weather-tools",
    "type": "mcp",
    "config": {
      "transport": "sse",
      "url": "https://weather-mcp.example.com/sse"
    }
  }' | jq .
```

## Step 4: OAuth 2.1 for Remote MCP Servers

Remote MCP servers can require OAuth 2.1 authentication. Configure the
OAuth flow in the plugin registration:

```toml
[[plugins]]
name = "secure-tools"
type = "mcp"

[plugins.config]
transport = "sse"
url = "https://secure-mcp.example.com/sse"

[plugins.config.oauth]
client_id = "aivyx-client"
client_secret_ref = "MCP_OAUTH_SECRET"
authorization_url = "https://auth.example.com/authorize"
token_url = "https://auth.example.com/token"
scopes = ["tools:read", "tools:execute"]
```

Aivyx handles the OAuth token lifecycle automatically:

1. Obtains an access token using the client credentials grant.
2. Attaches the token to all requests to the MCP server.
3. Refreshes the token before expiry.

## Step 5: Bidirectional Sampling

MCP supports bidirectional communication: the tool server can request LLM
completions from the agent via `sampling/createMessage`. This is useful when
a tool needs AI reasoning as part of its execution.

To handle sampling requests in your server, listen for `sampling/createMessage`
calls from the client and respond with the model's output:

```javascript
// In your handleRequest function, add:
case 'sampling/createMessage':
  // The client (Aivyx) sends this to your server when your tool
  // previously returned a sampling request.
  // Your server processes the model's response and continues.
  const modelResponse = params.content;
  // Use the model's response in your tool logic...
  return {
    jsonrpc: '2.0',
    id,
    result: { content: [{ type: 'text', text: 'Processed with AI assistance' }] },
  };
```

Note: sampling requires the agent to have the `sampling` capability enabled
in its profile.

## Step 6: Test with MCP Inspector

Use the MCP Inspector to test your server interactively:

```bash
npx @anthropic-ai/mcp-inspector
```

The inspector provides a web UI where you can:

- Connect to your MCP server (stdio or SSE).
- Browse available tools and their schemas.
- Invoke tools with test inputs.
- Inspect JSON-RPC messages.

For stdio servers, configure the inspector to launch your server:

```bash
npx @anthropic-ai/mcp-inspector --command "node /path/to/weather-mcp-server.js"
```

## Step 7: Assign MCP Tools to Agents

Once registered, assign the MCP tools to specific agents:

```bash
curl -s -X PATCH http://localhost:3000/agents/my-agent \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "plugins": ["weather-tools"]
  }' | jq .
```

The agent can now invoke `lookup_weather` as part of its tool repertoire.

## Best Practices

- **Validate inputs thoroughly**: MCP clients send JSON Schema-validated inputs,
  but always validate on the server side as well.
- **Return structured data**: prefer JSON in `text` content parts for machine-
  readable outputs. Agents can parse structured data more reliably.
- **Handle errors gracefully**: return JSON-RPC error responses with descriptive
  messages rather than crashing.
- **Keep tools focused**: one tool per action. Prefer `lookup_weather` and
  `lookup_forecast` over a single `weather` tool with a `mode` parameter.
- **Document thoroughly**: clear `description` fields in tool schemas help
  agents select the right tool for the task.
