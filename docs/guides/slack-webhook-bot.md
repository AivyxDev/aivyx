# Building a Slack Bot with Aivyx Webhooks

This guide walks through connecting a Slack workspace to Aivyx Engine so that
Slack messages trigger agent workflows and responses are posted back to channels.

## Prerequisites

- A Slack workspace where you have permission to create apps.
- Aivyx Engine running and accessible over HTTPS (Slack requires HTTPS for
  event subscriptions).
- A valid Aivyx bearer token for API authentication.
- `curl` and `jq` installed for testing.

## Step 1: Create a Slack App

1. Go to [api.slack.com/apps](https://api.slack.com/apps) and click
   **Create New App** > **From scratch**.
2. Name it (e.g., "Aivyx Agent") and select your workspace.
3. Under **OAuth & Permissions**, add these bot token scopes:
   - `chat:write` -- post messages
   - `app_mentions:read` -- receive @mentions
   - `channels:history` -- read channel messages (if needed)
4. Install the app to your workspace and copy the **Bot User OAuth Token**
   (starts with `xoxb-`).
5. Under **Basic Information**, copy the **Signing Secret** -- this is used
   for HMAC verification.

## Step 2: Configure the Aivyx Webhook Trigger

Add a webhook trigger to your `aivyx.toml` configuration:

```toml
[[triggers]]
name = "slack-events"
agent = "slack-responder"
prompt_template = "Process this Slack event and respond appropriately:\n{payload}"
secret_ref = "SLACK_SIGNING_SECRET"
enabled = true
```

Set the signing secret as an environment variable:

```bash
export SLACK_SIGNING_SECRET="your-slack-signing-secret"
```

Restart Aivyx Engine to pick up the new trigger configuration.

## Step 3: Create the Agent Profile

Create an agent profile with capabilities appropriate for a Slack bot:

```bash
curl -s -X POST http://localhost:3000/agents \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "slack-responder",
    "system_prompt": "You are a helpful assistant responding to Slack messages. Be concise and professional. Format responses using Slack markdown (bold with *text*, code with `backticks`).",
    "model": "claude-sonnet-4-20250514",
    "capabilities": {
      "shell": [],
      "filesystem": [],
      "network": ["api.slack.com:443"]
    }
  }' | jq .
```

Note that the agent has no shell or filesystem capabilities -- it can only
make network requests to Slack's API. This follows the principle of least
privilege (see ADR-0002).

## Step 4: Understand HMAC Verification

Aivyx verifies incoming webhook requests using HMAC-SHA256. Slack sends
the following headers with each event:

| Header                   | Purpose                              |
|--------------------------|--------------------------------------|
| `X-Slack-Request-Timestamp` | Unix timestamp of the request     |
| `X-Slack-Signature`       | HMAC signature (`v0=sha256(...)`)   |

Aivyx's webhook handler automatically:

1. Reads the `X-Slack-Request-Timestamp` header.
2. Rejects requests older than 5 minutes (replay protection).
3. Constructs the signing base string: `v0:{timestamp}:{body}`.
4. Computes HMAC-SHA256 using the `SLACK_SIGNING_SECRET`.
5. Compares the computed signature with the `X-Slack-Signature` header.

No additional configuration is needed -- the `secret_ref` in the trigger
config handles this automatically.

## Step 5: Test the Webhook Locally

Before pointing Slack to your endpoint, test with a simulated event:

```bash
TIMESTAMP=$(date +%s)
BODY='{"type":"event_callback","event":{"type":"app_mention","text":"<@U123> what is the weather?","channel":"C456","user":"U789"}}'
SIG_BASE="v0:${TIMESTAMP}:${BODY}"
SIGNATURE="v0=$(echo -n "$SIG_BASE" | openssl dgst -sha256 -hmac "$SLACK_SIGNING_SECRET" | cut -d' ' -f2)"

curl -s -X POST http://localhost:3000/webhooks/slack-events \
  -H "Content-Type: application/json" \
  -H "X-Slack-Request-Timestamp: $TIMESTAMP" \
  -H "X-Slack-Signature: $SIGNATURE" \
  -d "$BODY" | jq .
```

You should receive a `200 OK` response. Check the Aivyx Engine logs for the
agent's processing output.

## Step 6: Handle Slack's URL Verification Challenge

When you first configure the event subscription URL, Slack sends a
verification challenge:

```json
{
  "type": "url_verification",
  "challenge": "random-challenge-string",
  "token": "deprecated-verification-token"
}
```

Aivyx automatically responds to `url_verification` events by echoing the
challenge value. No agent processing occurs for these requests.

## Step 7: Configure Slack Event Subscriptions

1. In your Slack app settings, go to **Event Subscriptions**.
2. Toggle **Enable Events** to on.
3. Set the **Request URL** to:
   ```
   https://your-domain.com/webhooks/slack-events
   ```
4. Wait for Slack to verify the URL (it sends the challenge from Step 6).
5. Under **Subscribe to bot events**, add:
   - `app_mention` -- triggers when someone @mentions the bot
   - `message.im` -- triggers on direct messages to the bot
6. Click **Save Changes**.

## Step 8: Post Responses Back to Slack

To have the agent post responses back to the Slack channel, configure a
response webhook in the agent's tool set. Add a `slack_post` tool that calls
the Slack API:

```bash
curl -s -X POST http://localhost:3000/plugins \
  -H "Authorization: Bearer $AIVYX_TOKEN" \
  -H "Content-Type: application/json" \
  -d '{
    "name": "slack-post",
    "type": "http",
    "config": {
      "url": "https://slack.com/api/chat.postMessage",
      "method": "POST",
      "headers": {
        "Authorization": "Bearer xoxb-your-bot-token"
      }
    }
  }' | jq .
```

## Troubleshooting

| Problem                        | Solution                                   |
|--------------------------------|--------------------------------------------|
| Slack shows "URL not verified" | Ensure HTTPS is configured and the endpoint is reachable from the internet |
| 401 on webhook requests        | Check that `SLACK_SIGNING_SECRET` matches the value in Slack app settings |
| Agent does not respond         | Check Aivyx logs for capability denials or model errors |
| Duplicate messages             | Slack retries after 3 seconds; ensure your response returns within 3s or use a queue |
