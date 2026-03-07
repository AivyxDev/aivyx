# Building a Discord Bot with the A2A Protocol

This guide covers building a Discord bot that communicates with Aivyx Engine
using the Agent-to-Agent (A2A) protocol, an open standard for inter-agent
communication over JSON-RPC 2.0.

## Overview of the A2A Protocol

The A2A protocol defines a standard way for agents to discover each other's
capabilities and exchange tasks. Key concepts:

- **Agent Card**: a JSON document describing an agent's capabilities, published
  at a well-known URL.
- **Tasks**: units of work sent from one agent to another, with lifecycle states
  (submitted, working, completed, failed, canceled).
- **Artifacts**: outputs produced by a task (text, files, structured data).
- **Push Notifications**: webhook callbacks for async task completion.

A2A uses JSON-RPC 2.0 as its transport format over HTTP.

## Step 1: Discover the Agent Card

Every A2A-compatible agent publishes its capabilities at
`/.well-known/agent.json`:

```bash
curl -s http://localhost:3000/.well-known/agent.json | jq .
```

Response:

```json
{
  "name": "Aivyx Engine",
  "description": "AI agent engine with multi-agent collaboration",
  "url": "http://localhost:3000/a2a",
  "version": "0.7.4",
  "capabilities": {
    "streaming": true,
    "pushNotifications": true,
    "stateTransitionHistory": true
  },
  "skills": [
    {
      "id": "general-chat",
      "name": "General Chat",
      "description": "General-purpose conversational AI"
    }
  ],
  "authentication": {
    "schemes": ["bearer"]
  }
}
```

The `url` field tells clients where to send JSON-RPC requests.

## Step 2: Send a Task

Tasks are sent via `POST /a2a` using the `tasks/send` method:

```bash
curl -s -X POST http://localhost:3000/a2a \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-001",
    "method": "tasks/send",
    "params": {
      "id": "task-discord-123",
      "message": {
        "role": "user",
        "parts": [
          { "type": "text", "text": "Summarize the key points of quantum computing." }
        ]
      }
    }
  }' | jq .
```

Response:

```json
{
  "jsonrpc": "2.0",
  "id": "req-001",
  "result": {
    "id": "task-discord-123",
    "status": {
      "state": "completed",
      "message": {
        "role": "agent",
        "parts": [
          { "type": "text", "text": "Here are the key points..." }
        ]
      }
    },
    "artifacts": []
  }
}
```

## Step 3: Poll for Results

For long-running tasks, the initial response may return `state: "working"`.
Poll with `tasks/get`:

```bash
curl -s -X POST http://localhost:3000/a2a \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-002",
    "method": "tasks/get",
    "params": {
      "id": "task-discord-123"
    }
  }' | jq .result.status.state
```

Task states: `submitted` -> `working` -> `completed` | `failed` | `canceled`.

## Step 4: Stream Responses with SSE

For real-time streaming, use the `POST /a2a/stream` endpoint with
`tasks/sendSubscribe`:

```bash
curl -s -N -X POST http://localhost:3000/a2a/stream \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-003",
    "method": "tasks/sendSubscribe",
    "params": {
      "id": "task-discord-456",
      "message": {
        "role": "user",
        "parts": [
          { "type": "text", "text": "Write a haiku about Rust." }
        ]
      }
    }
  }'
```

The response is a Server-Sent Events (SSE) stream:

```
data: {"jsonrpc":"2.0","method":"tasks/status","params":{"id":"task-discord-456","status":{"state":"working"}}}

data: {"jsonrpc":"2.0","method":"tasks/artifact","params":{"id":"task-discord-456","artifact":{"parts":[{"type":"text","text":"Memory safe and fast"}]}}}

data: {"jsonrpc":"2.0","method":"tasks/status","params":{"id":"task-discord-456","status":{"state":"completed"}}}
```

## Step 5: Build the Discord Bot

Here is a Discord.js bot that bridges Discord messages to A2A tasks:

```javascript
const { Client, GatewayIntentBits } = require('discord.js');

const client = new Client({
  intents: [
    GatewayIntentBits.Guilds,
    GatewayIntentBits.GuildMessages,
    GatewayIntentBits.MessageContent,
  ],
});

const AIVYX_URL = process.env.AIVYX_URL || 'http://localhost:3000';
const AIVYX_TOKEN = process.env.AIVYX_TOKEN;

client.on('messageCreate', async (message) => {
  // Ignore bot messages and messages that don't mention the bot
  if (message.author.bot) return;
  if (!message.mentions.has(client.user)) return;

  const userText = message.content.replace(/<@!?\d+>/g, '').trim();
  if (!userText) return;

  await message.channel.sendTyping();

  try {
    const taskId = `discord-${message.id}`;
    const response = await fetch(`${AIVYX_URL}/a2a`, {
      method: 'POST',
      headers: {
        'Authorization': `Bearer ${AIVYX_TOKEN}`,
        'Content-Type': 'application/json',
      },
      body: JSON.stringify({
        jsonrpc: '2.0',
        id: taskId,
        method: 'tasks/send',
        params: {
          id: taskId,
          message: {
            role: 'user',
            parts: [{ type: 'text', text: userText }],
          },
        },
      }),
    });

    const data = await response.json();

    if (data.result?.status?.state === 'completed') {
      const text = data.result.status.message.parts
        .filter((p) => p.type === 'text')
        .map((p) => p.text)
        .join('\n');

      // Discord has a 2000 character limit
      if (text.length > 2000) {
        const chunks = text.match(/[\s\S]{1,2000}/g);
        for (const chunk of chunks) {
          await message.reply(chunk);
        }
      } else {
        await message.reply(text);
      }
    } else {
      await message.reply('The agent is still working on your request. Please wait.');
    }
  } catch (error) {
    console.error('A2A request failed:', error);
    await message.reply('Sorry, I encountered an error processing your request.');
  }
});

client.login(process.env.DISCORD_TOKEN);
```

## Step 6: Push Notifications for Async Tasks

For tasks that take longer, set up push notifications so Aivyx calls back
when the task completes:

```bash
curl -s -X POST http://localhost:3000/a2a \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "jsonrpc": "2.0",
    "id": "req-004",
    "method": "tasks/send",
    "params": {
      "id": "task-long-running",
      "message": {
        "role": "user",
        "parts": [{ "type": "text", "text": "Analyze this large codebase." }]
      },
      "pushNotification": {
        "url": "https://your-bot-server.com/a2a-callback",
        "authentication": {
          "scheme": "bearer",
          "token": "your-callback-secret"
        }
      }
    }
  }' | jq .
```

When the task completes, Aivyx sends a POST to your callback URL with the
task result. Your bot server can then post the result to the appropriate
Discord channel.

## Running the Bot

```bash
export DISCORD_TOKEN="your-discord-bot-token"
export AIVYX_URL="http://localhost:3000"
export AIVYX_TOKEN="your-aivyx-bearer-token"
node bot.js
```

## Troubleshooting

| Problem                    | Solution                                         |
|----------------------------|--------------------------------------------------|
| Bot does not respond       | Ensure `MessageContent` intent is enabled in the Discord developer portal |
| 401 from Aivyx             | Verify `AIVYX_TOKEN` is set and valid            |
| Timeout on long tasks      | Use push notifications or SSE streaming instead of synchronous `tasks/send` |
| Response truncated         | The bot splits messages at 2000 characters; check for edge cases in splitting logic |
