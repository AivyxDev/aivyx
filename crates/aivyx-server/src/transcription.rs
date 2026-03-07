//! Speech-to-text transcription via OpenAI Whisper API or Ollama.
//!
//! Provides [`transcribe()`] which sends audio bytes to the configured
//! speech provider and returns the transcribed text.

use std::path::Path;

use aivyx_config::speech::{SpeechConfig, SpeechProvider};
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{EncryptedStore, MasterKey};
use serde::Deserialize;

/// Result of a transcription operation.
pub struct TranscriptionResult {
    /// The transcribed text.
    pub text: String,
    /// Duration of the audio in seconds (if reported by the provider).
    pub duration_secs: Option<f64>,
}

/// Transcribe audio bytes to text using the configured speech provider.
///
/// # Arguments
/// * `config` — Speech provider configuration
/// * `audio_bytes` — Raw audio data (wav, mp3, etc.)
/// * `filename` — Original filename (used for MIME type detection)
/// * `master_key` — For decrypting the API key from the encrypted store
/// * `store_path` — Path to the encrypted store database
pub async fn transcribe(
    config: &SpeechConfig,
    audio_bytes: Vec<u8>,
    filename: &str,
    master_key: &MasterKey,
    store_path: &Path,
) -> Result<TranscriptionResult> {
    match &config.provider {
        SpeechProvider::OpenAi { api_key_ref } => {
            transcribe_openai(
                &config.model,
                api_key_ref,
                audio_bytes,
                filename,
                master_key,
                store_path,
            )
            .await
        }
        SpeechProvider::Ollama { base_url } => {
            let base = base_url.as_deref().unwrap_or("http://localhost:11434");
            transcribe_ollama(base, &config.model, audio_bytes).await
        }
    }
}

/// OpenAI Whisper API response format.
#[derive(Debug, Deserialize)]
struct WhisperResponse {
    text: String,
    #[serde(default)]
    duration: Option<f64>,
}

/// Transcribe via OpenAI's Whisper API.
async fn transcribe_openai(
    model: &str,
    api_key_ref: &str,
    audio_bytes: Vec<u8>,
    filename: &str,
    master_key: &MasterKey,
    store_path: &Path,
) -> Result<TranscriptionResult> {
    // Resolve the API key from the encrypted store
    let store = EncryptedStore::open(store_path)?;
    let key_bytes = store.get(api_key_ref, master_key)?.ok_or_else(|| {
        AivyxError::Config(format!(
            "speech API key '{api_key_ref}' not found in encrypted store"
        ))
    })?;
    let api_key = String::from_utf8(key_bytes)
        .map_err(|_| AivyxError::Config("speech API key is not valid UTF-8".into()))?;

    let client = reqwest::Client::new();

    // Build multipart form
    let file_part = reqwest::multipart::Part::bytes(audio_bytes)
        .file_name(filename.to_string())
        .mime_str(guess_audio_mime(filename))
        .map_err(|e| AivyxError::Http(e.to_string()))?;

    let form = reqwest::multipart::Form::new()
        .text("model", model.to_string())
        .text("response_format", "verbose_json")
        .part("file", file_part);

    let resp = client
        .post("https://api.openai.com/v1/audio/transcriptions")
        .bearer_auth(api_key)
        .multipart(form)
        .send()
        .await
        .map_err(|e| AivyxError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
        return Err(AivyxError::LlmProvider(format!(
            "OpenAI Whisper API error ({status}): {body}"
        )));
    }

    let whisper: WhisperResponse = resp
        .json()
        .await
        .map_err(|e| AivyxError::Http(format!("failed to parse Whisper response: {e}")))?;

    Ok(TranscriptionResult {
        text: whisper.text,
        duration_secs: whisper.duration,
    })
}

/// Transcribe via Ollama's speech model (best-effort).
///
/// Ollama's Whisper support is experimental. This sends the audio as
/// base64-encoded content to the generate endpoint.
async fn transcribe_ollama(
    base_url: &str,
    model: &str,
    audio_bytes: Vec<u8>,
) -> Result<TranscriptionResult> {
    use base64::Engine;
    let audio_b64 = base64::engine::general_purpose::STANDARD.encode(&audio_bytes);

    let client = reqwest::Client::new();
    let resp = client
        .post(format!("{base_url}/api/generate"))
        .json(&serde_json::json!({
            "model": model,
            "prompt": "Transcribe the following audio to text.",
            "images": [audio_b64],
            "stream": false,
        }))
        .send()
        .await
        .map_err(|e| AivyxError::Http(e.to_string()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_else(|_| "unknown error".into());
        return Err(AivyxError::LlmProvider(format!(
            "Ollama transcription error ({status}): {body}"
        )));
    }

    let body: serde_json::Value = resp
        .json()
        .await
        .map_err(|e| AivyxError::Http(format!("failed to parse Ollama response: {e}")))?;

    let text = body["response"].as_str().unwrap_or("").to_string();

    Ok(TranscriptionResult {
        text,
        duration_secs: None,
    })
}

/// Guess the MIME type for an audio file based on its extension.
fn guess_audio_mime(filename: &str) -> &'static str {
    let ext = filename.rsplit('.').next().unwrap_or("");
    match ext.to_lowercase().as_str() {
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "webm" => "audio/webm",
        "m4a" => "audio/mp4",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn audio_mime_detection() {
        assert_eq!(guess_audio_mime("recording.mp3"), "audio/mpeg");
        assert_eq!(guess_audio_mime("audio.wav"), "audio/wav");
        assert_eq!(guess_audio_mime("voice.flac"), "audio/flac");
        assert_eq!(guess_audio_mime("unknown.xyz"), "application/octet-stream");
        assert_eq!(guess_audio_mime("meeting.m4a"), "audio/mp4");
    }
}
