# WebSocket Protocol Guide

This guide covers the Aivyx Engine WebSocket protocol for real-time,
bidirectional communication with agents. Use WebSockets when you need
streaming responses, tool execution visibility, or human-in-the-loop
approval flows.

## Connecting

Connect to the WebSocket endpoint:

```
ws://localhost:3000/ws
```

For TLS deployments:

```
wss://aivyx.example.com/ws
```

## Authentication

The first message sent on a new WebSocket connection **must** be an
authentication message. Any other message type sent before authentication
will result in an `auth_error` and connection closure.

```json
{"type": "auth", "token": "your-bearer-token"}
```

### Success response

```json
{"type": "auth_success", "session_id": "ses_01HABC..."}
```

### Error response

```json
{"type": "auth_error", "message": "Invalid or expired token"}
```

After `auth_error`, the server closes the connection.

## Client to Server Messages

### `message` -- Send a chat message

```json
{
  "type": "message",
  "agent": "default",
  "content": "Explain how async/await works in Rust.",
  "session_id": "ses_01HABC..."
}
```

Omit `session_id` to start a new session. Include it to continue an existing
conversation.

#### With images

```json
{
  "type": "message",
  "agent": "default",
  "content": "What does this diagram show?",
  "images": [
    {
      "media_type": "image/png",
      "data": "base64-encoded-image-data"
    }
  ]
}
```

### `approval_response` -- Respond to an approval request

When the server sends an `approval_request` (see below), the client responds
with:

```json
{
  "type": "approval_response",
  "request_id": "apr_01H...",
  "approved": true,
  "reason": "Looks good, proceed."
}
```

Set `approved: false` to deny the action:

```json
{
  "type": "approval_response",
  "request_id": "apr_01H...",
  "approved": false,
  "reason": "Do not delete that file."
}
```

## Server to Client Messages

### `text` -- Streamed text chunk

Sent as the agent generates its response. Concatenate all `text` messages
to build the full response.

```json
{
  "type": "text",
  "content": "Async/await in Rust works by transforming"
}
```

### `thinking` -- Agent reasoning

Sent when extended thinking is enabled for the agent. Shows the agent's
internal reasoning process.

```json
{
  "type": "thinking",
  "content": "The user is asking about async/await. I should explain the state machine transformation..."
}
```

### `tool_call` -- Tool invocation started

```json
{
  "type": "tool_call",
  "id": "tc_01H...",
  "name": "shell",
  "input": {
    "command": "cargo test"
  }
}
```

### `tool_result` -- Tool invocation completed

```json
{
  "type": "tool_result",
  "id": "tc_01H...",
  "name": "shell",
  "output": "running 42 tests\ntest result: ok. 42 passed; 0 failed",
  "success": true
}
```

### `approval_request` -- Human-in-the-loop

Sent when the agent wants to perform an action that requires user approval
(e.g., shell commands, file modifications):

```json
{
  "type": "approval_request",
  "request_id": "apr_01H...",
  "action": "shell",
  "description": "Run: rm -rf /tmp/build-cache",
  "risk_level": "high"
}
```

The agent pauses until the client sends an `approval_response`.

### `done` -- Turn complete

Sent when the agent has finished its response for this turn:

```json
{
  "type": "done",
  "session_id": "ses_01HABC...",
  "usage": {
    "input_tokens": 150,
    "output_tokens": 420,
    "cost_usd": 0.0058
  }
}
```

### `error` -- Error occurred

```json
{
  "type": "error",
  "message": "Agent 'nonexistent' not found",
  "code": "agent_not_found"
}
```

Errors do not close the connection unless they are authentication errors.
The client can send a new message after receiving an error.

## Message Flow Example

```
Client                              Server
  |                                    |
  |-- auth {token} ------------------->|
  |<-------------- auth_success -------|
  |                                    |
  |-- message {content} -------------->|
  |<-------------- thinking ---------- |
  |<-------------- text --------------|
  |<-------------- text --------------|
  |<-------------- tool_call ---------|
  |<-------------- approval_request --|
  |                                    |
  |-- approval_response {approved} --->|
  |<-------------- tool_result -------|
  |<-------------- text --------------|
  |<-------------- done --------------|
```

## Example: Python WebSocket Client

```python
import asyncio
import json
import websockets

async def main():
    uri = "ws://localhost:3000/ws"
    token = "your-bearer-token"

    async with websockets.connect(uri) as ws:
        # Authenticate
        await ws.send(json.dumps({"type": "auth", "token": token}))
        auth_response = json.loads(await ws.recv())

        if auth_response["type"] != "auth_success":
            print(f"Auth failed: {auth_response.get('message')}")
            return

        session_id = auth_response["session_id"]
        print(f"Connected. Session: {session_id}")

        # Send a message
        await ws.send(json.dumps({
            "type": "message",
            "agent": "default",
            "content": "What is the Rust borrow checker?",
            "session_id": session_id,
        }))

        # Process responses
        full_response = ""
        async for raw in ws:
            msg = json.loads(raw)

            if msg["type"] == "text":
                full_response += msg["content"]
                print(msg["content"], end="", flush=True)

            elif msg["type"] == "tool_call":
                print(f"\n[Tool: {msg['name']}({msg['input']})]")

            elif msg["type"] == "tool_result":
                print(f"[Result: {msg['output'][:100]}...]")

            elif msg["type"] == "approval_request":
                print(f"\n[Approval needed: {msg['description']}]")
                user_input = input("Approve? (y/n): ")
                await ws.send(json.dumps({
                    "type": "approval_response",
                    "request_id": msg["request_id"],
                    "approved": user_input.lower() == "y",
                }))

            elif msg["type"] == "done":
                print(f"\n\nDone. Tokens: {msg['usage']}")
                break

            elif msg["type"] == "error":
                print(f"\nError: {msg['message']}")
                break

asyncio.run(main())
```

## Example: TypeScript WebSocket Client

```typescript
const ws = new WebSocket("ws://localhost:3000/ws");
const TOKEN = "your-bearer-token";

ws.onopen = () => {
  ws.send(JSON.stringify({ type: "auth", token: TOKEN }));
};

ws.onmessage = (event) => {
  const msg = JSON.parse(event.data);

  switch (msg.type) {
    case "auth_success":
      console.log(`Connected. Session: ${msg.session_id}`);
      // Send a message after auth
      ws.send(
        JSON.stringify({
          type: "message",
          agent: "default",
          content: "Hello, world!",
          session_id: msg.session_id,
        })
      );
      break;

    case "auth_error":
      console.error(`Auth failed: ${msg.message}`);
      ws.close();
      break;

    case "text":
      process.stdout.write(msg.content);
      break;

    case "tool_call":
      console.log(`\n[Tool: ${msg.name}]`);
      break;

    case "tool_result":
      console.log(`[Result: ${msg.output.substring(0, 100)}]`);
      break;

    case "approval_request":
      console.log(`\n[Approval needed: ${msg.description}]`);
      // In a real app, present a UI for approval
      ws.send(
        JSON.stringify({
          type: "approval_response",
          request_id: msg.request_id,
          approved: true,
        })
      );
      break;

    case "done":
      console.log(`\nDone. Cost: $${msg.usage.cost_usd}`);
      break;

    case "error":
      console.error(`Error: ${msg.message}`);
      break;
  }
};

ws.onerror = (error) => {
  console.error("WebSocket error:", error);
};

ws.onclose = () => {
  console.log("Connection closed.");
};
```

## Reconnection Strategy

WebSocket connections can drop due to network issues, server restarts, or
idle timeouts. Implement reconnection with exponential backoff:

1. On unexpected close, wait 1 second and reconnect.
2. If reconnection fails, double the wait time (2s, 4s, 8s, ...).
3. Cap the maximum wait at 30 seconds.
4. On successful reconnection, reset the wait time to 1 second.
5. Re-authenticate after reconnecting.
6. Use the same `session_id` to resume the conversation.

The server retains session state for 30 minutes after disconnection (by
default). After that, the session is garbage-collected and a new session must
be started.
