# Voice WebSocket Protocol Guide

This guide covers the Aivyx Engine voice WebSocket protocol for real-time
speech-to-text (STT), agent processing, and text-to-speech (TTS) over a
single persistent connection.

## Connecting

Connect to the voice WebSocket endpoint:

```
ws://localhost:3000/ws/voice
```

For TLS deployments:

```
wss://aivyx.example.com/ws/voice
```

## State Machine

The voice connection progresses through these states:

```
Authenticating --> Ready --> Listening --> Processing --> Speaking
                    ^                                       |
                    |                                       |
                    +---------------------------------------+
```

| State           | Description                                    |
|-----------------|------------------------------------------------|
| Authenticating  | Waiting for auth message                       |
| Ready           | Authenticated, waiting for audio or config     |
| Listening       | Receiving audio frames from the client         |
| Processing      | STT complete, agent is generating a response   |
| Speaking        | TTS audio is being streamed back to the client |

After `Speaking` completes, the connection returns to `Ready` for the next
turn.

## Authentication

The first message must be an auth message (identical to the standard
WebSocket protocol):

```json
{"type": "auth", "token": "your-bearer-token"}
```

### Success

```json
{"type": "auth_success", "session_id": "ses_01HABC..."}
```

### Error

```json
{"type": "auth_error", "message": "Invalid or expired token"}
```

After authentication, the server sends a `ready` message:

```json
{"type": "ready", "state": "ready"}
```

## Client to Server Messages

### `config` -- Set voice configuration

Send before starting audio to configure STT and TTS providers:

```json
{
  "type": "config",
  "stt": {
    "provider": "whisper",
    "model": "whisper-1",
    "language": "en"
  },
  "tts": {
    "provider": "openai",
    "model": "tts-1",
    "voice": "alloy",
    "speed": 1.0
  },
  "agent": "default",
  "vad": {
    "enabled": true,
    "silence_threshold_ms": 800
  }
}
```

Configuration fields:

| Field                      | Default       | Description                   |
|----------------------------|---------------|-------------------------------|
| `stt.provider`             | `"whisper"`   | STT provider                  |
| `stt.model`                | `"whisper-1"` | STT model                     |
| `stt.language`             | `"en"`        | Language code (ISO 639-1)     |
| `tts.provider`             | `"openai"`    | TTS provider                  |
| `tts.model`                | `"tts-1"`     | TTS model                     |
| `tts.voice`                | `"alloy"`     | Voice selection               |
| `tts.speed`                | `1.0`         | Playback speed (0.5 - 2.0)   |
| `agent`                    | `"default"`   | Agent to use for processing   |
| `vad.enabled`              | `true`        | Voice Activity Detection      |
| `vad.silence_threshold_ms` | `800`         | Silence duration to end turn  |

### Binary frames -- Audio input

Send raw audio data as binary WebSocket frames. Supported formats:

| Format            | Details                          |
|-------------------|----------------------------------|
| PCM 16-bit        | 16 kHz, mono, little-endian      |
| MP3               | Any bitrate (transcoded server-side) |

The server uses Voice Activity Detection (VAD) to determine when the user
has finished speaking. You can also explicitly signal the end of audio input.

### `end_audio` -- Signal end of audio

Explicitly tell the server that audio input is complete (useful when VAD is
disabled or for push-to-talk interfaces):

```json
{"type": "end_audio"}
```

### `interrupt` -- Barge-in

Send an interrupt to stop the current TTS output. The server immediately
stops sending audio frames and transitions back to `Ready`:

```json
{"type": "interrupt"}
```

This enables natural conversational barge-in, where the user starts speaking
while the agent is still talking.

## Server to Client Messages

### `ready` -- Ready for audio

Indicates the server is ready to receive audio:

```json
{"type": "ready", "state": "ready"}
```

### `transcript` -- STT result

The transcribed text from the user's audio input:

```json
{
  "type": "transcript",
  "text": "What is the weather like in London?",
  "language": "en",
  "confidence": 0.95
}
```

### `text` -- Agent response text

The agent's text response (sent before or alongside TTS audio):

```json
{
  "type": "text",
  "content": "The current weather in London is partly cloudy with a temperature of 18 degrees Celsius."
}
```

### Binary frames -- TTS audio output

The server sends TTS audio as binary WebSocket frames. The format matches
the configured TTS output:

| Format    | Details                           |
|-----------|-----------------------------------|
| PCM 16-bit | 24 kHz, mono, little-endian      |
| MP3       | 128 kbps (configurable)           |

Frames are sent incrementally as TTS generation progresses, enabling
low-latency playback.

### `speaking_done` -- TTS complete

Sent when all TTS audio frames have been delivered:

```json
{
  "type": "speaking_done",
  "usage": {
    "stt_seconds": 3.2,
    "tts_characters": 95,
    "agent_input_tokens": 120,
    "agent_output_tokens": 45,
    "cost_usd": 0.0082
  }
}
```

After `speaking_done`, the connection returns to `Ready` for the next turn.

### `error` -- Error occurred

```json
{
  "type": "error",
  "message": "STT transcription failed: audio too short",
  "code": "stt_error"
}
```

## Audio Format Requirements

### Input (client to server)

- **PCM**: 16-bit signed integers, 16 kHz sample rate, mono channel,
  little-endian byte order. Frame size: send chunks of 3200 bytes (100ms of
  audio) for optimal latency.
- **MP3**: any standard MP3 encoding. The server transcodes internally.

### Output (server to client)

- **PCM**: 16-bit signed integers, 24 kHz sample rate, mono channel,
  little-endian byte order.
- **MP3**: 128 kbps mono (configurable via the `config` message).

## Message Flow Example

```
Client                               Server
  |                                     |
  |-- auth {token} -------------------->|
  |<------------- auth_success ---------|
  |<------------- ready ---------------|
  |                                     |
  |-- config {stt, tts, agent} -------->|
  |                                     |
  |-- [binary: audio chunk 1] --------->|
  |-- [binary: audio chunk 2] --------->|
  |-- [binary: audio chunk 3] --------->|
  |   (VAD detects silence)             |
  |                                     |
  |<------------- transcript ----------|
  |<------------- text ----------------|
  |<------------- [binary: TTS audio] -|
  |<------------- [binary: TTS audio] -|
  |<------------- [binary: TTS audio] -|
  |<------------- speaking_done -------|
  |<------------- ready ---------------|
  |                                     |
  |   (next turn...)                    |
```

## Example: Python Audio Streaming Client

```python
import asyncio
import json
import struct
import websockets
import pyaudio

SAMPLE_RATE = 16000
CHANNELS = 1
CHUNK_SIZE = 3200  # 100ms at 16kHz, 16-bit mono
FORMAT = pyaudio.paInt16

async def voice_session():
    uri = "ws://localhost:3000/ws/voice"
    token = "your-bearer-token"

    async with websockets.connect(uri) as ws:
        # Authenticate
        await ws.send(json.dumps({"type": "auth", "token": token}))
        auth = json.loads(await ws.recv())
        if auth["type"] != "auth_success":
            print(f"Auth failed: {auth.get('message')}")
            return

        # Wait for ready
        ready = json.loads(await ws.recv())
        assert ready["type"] == "ready"

        # Configure voice
        await ws.send(json.dumps({
            "type": "config",
            "stt": {"provider": "whisper", "model": "whisper-1", "language": "en"},
            "tts": {"provider": "openai", "model": "tts-1", "voice": "alloy"},
            "agent": "default",
        }))

        # Set up audio input
        pa = pyaudio.PyAudio()
        input_stream = pa.open(
            format=FORMAT,
            channels=CHANNELS,
            rate=SAMPLE_RATE,
            input=True,
            frames_per_buffer=CHUNK_SIZE // 2,  # 16-bit = 2 bytes per sample
        )

        # Set up audio output
        output_stream = pa.open(
            format=FORMAT,
            channels=CHANNELS,
            rate=24000,  # TTS output is 24kHz
            output=True,
        )

        print("Listening... Speak now.")

        # Send audio in a background task
        async def send_audio():
            while True:
                audio_data = input_stream.read(CHUNK_SIZE // 2, exception_on_overflow=False)
                await ws.send(audio_data)
                await asyncio.sleep(0.01)

        send_task = asyncio.create_task(send_audio())

        # Receive responses
        try:
            async for raw in ws:
                if isinstance(raw, bytes):
                    # TTS audio output -- play it
                    output_stream.write(raw)
                else:
                    msg = json.loads(raw)
                    if msg["type"] == "transcript":
                        print(f"\nYou said: {msg['text']}")
                    elif msg["type"] == "text":
                        print(f"Agent: {msg['content']}")
                    elif msg["type"] == "speaking_done":
                        print(f"[Turn complete. Cost: ${msg['usage']['cost_usd']:.4f}]")
                    elif msg["type"] == "error":
                        print(f"Error: {msg['message']}")
        finally:
            send_task.cancel()
            input_stream.close()
            output_stream.close()
            pa.terminate()

asyncio.run(voice_session())
```

## Example: Browser MediaRecorder + WebSocket

```html
<!DOCTYPE html>
<html>
<head><title>Aivyx Voice</title></head>
<body>
  <button id="start">Start</button>
  <button id="stop" disabled>Stop</button>
  <button id="interrupt">Interrupt</button>
  <div id="transcript"></div>

  <script>
    const TOKEN = "your-bearer-token";
    let ws, mediaRecorder, audioContext, audioQueue = [];

    document.getElementById("start").onclick = async () => {
      ws = new WebSocket("ws://localhost:3000/ws/voice");

      ws.onopen = () => {
        ws.send(JSON.stringify({ type: "auth", token: TOKEN }));
      };

      ws.onmessage = async (event) => {
        if (event.data instanceof Blob) {
          // TTS audio -- decode and play
          const arrayBuffer = await event.data.arrayBuffer();
          playAudio(arrayBuffer);
          return;
        }

        const msg = JSON.parse(event.data);

        switch (msg.type) {
          case "auth_success":
            ws.send(JSON.stringify({
              type: "config",
              stt: { provider: "whisper", model: "whisper-1", language: "en" },
              tts: { provider: "openai", model: "tts-1", voice: "alloy" },
              agent: "default",
            }));
            break;

          case "ready":
            startRecording();
            break;

          case "transcript":
            document.getElementById("transcript").innerHTML +=
              `<p><strong>You:</strong> ${msg.text}</p>`;
            break;

          case "text":
            document.getElementById("transcript").innerHTML +=
              `<p><strong>Agent:</strong> ${msg.content}</p>`;
            break;

          case "speaking_done":
            console.log("Turn complete:", msg.usage);
            break;
        }
      };

      document.getElementById("start").disabled = true;
      document.getElementById("stop").disabled = false;
    };

    document.getElementById("stop").onclick = () => {
      if (mediaRecorder && mediaRecorder.state === "recording") {
        mediaRecorder.stop();
      }
      ws.send(JSON.stringify({ type: "end_audio" }));
      document.getElementById("start").disabled = false;
      document.getElementById("stop").disabled = true;
    };

    document.getElementById("interrupt").onclick = () => {
      ws.send(JSON.stringify({ type: "interrupt" }));
    };

    async function startRecording() {
      const stream = await navigator.mediaDevices.getUserMedia({ audio: true });
      mediaRecorder = new MediaRecorder(stream, { mimeType: "audio/webm" });

      mediaRecorder.ondataavailable = (event) => {
        if (event.data.size > 0 && ws.readyState === WebSocket.OPEN) {
          ws.send(event.data);
        }
      };

      mediaRecorder.start(100); // Send chunks every 100ms
    }

    async function playAudio(arrayBuffer) {
      if (!audioContext) {
        audioContext = new AudioContext({ sampleRate: 24000 });
      }
      const audioBuffer = await audioContext.decodeAudioData(arrayBuffer);
      const source = audioContext.createBufferSource();
      source.buffer = audioBuffer;
      source.connect(audioContext.destination);
      source.start();
    }
  </script>
</body>
</html>
```

## Latency Optimization Tips

- **Send small audio chunks**: 100ms chunks (3200 bytes at 16kHz/16-bit)
  provide a good balance between latency and overhead.
- **Enable VAD**: voice activity detection eliminates the need for manual
  end-of-speech signaling, reducing turn-taking latency.
- **Use PCM over MP3**: PCM avoids encoding/decoding latency on both client
  and server.
- **Pre-buffer TTS output**: start playback as soon as the first audio frame
  arrives rather than waiting for `speaking_done`.
- **Use barge-in**: send `interrupt` when the user starts speaking to
  immediately stop TTS output and begin the next turn.
