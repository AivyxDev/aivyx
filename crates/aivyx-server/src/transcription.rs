//! Speech-to-text transcription via the [`SttProvider`] trait.
//!
//! Provides [`transcribe()`] which creates an STT provider from config
//! and transcribes audio bytes to text.

use std::path::Path;

use aivyx_config::speech::SpeechConfig;
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey};
use aivyx_llm::{AudioFormat, create_stt_provider};

/// Result of a transcription operation (re-export for server use).
pub use aivyx_llm::TranscriptionResult;

/// Transcribe audio bytes to text using the configured speech provider.
///
/// Creates an `SttProvider` from config and delegates to its `transcribe()` method.
pub async fn transcribe(
    config: &SpeechConfig,
    audio_bytes: Vec<u8>,
    filename: &str,
    master_key: &MasterKey,
    store_path: &Path,
) -> Result<TranscriptionResult> {
    let store = EncryptedStore::open(store_path)?;
    let provider = create_stt_provider(config, &store, master_key)?;
    let format = AudioFormat::from_filename(filename);
    provider.transcribe(&audio_bytes, format).await
}

#[cfg(test)]
mod tests {
    use aivyx_llm::AudioFormat;

    #[test]
    fn audio_format_from_filename() {
        assert_eq!(AudioFormat::from_filename("recording.mp3"), AudioFormat::Mp3);
        assert_eq!(AudioFormat::from_filename("audio.wav"), AudioFormat::Wav);
        assert_eq!(AudioFormat::from_filename("voice.flac"), AudioFormat::Flac);
        assert_eq!(
            AudioFormat::from_filename("unknown.xyz"),
            AudioFormat::Unknown
        );
        assert_eq!(AudioFormat::from_filename("meeting.m4a"), AudioFormat::M4a);
    }
}
