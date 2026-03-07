use async_trait::async_trait;

use aivyx_core::{AivyxError, ChannelAdapter, Result};

/// CLI-based channel adapter for interactive user approval.
///
/// Sends messages to stdout and reads responses from stdin.
/// Used for Leash-tier agents that need user approval before
/// executing tool calls.
pub struct CliChannel;

#[async_trait]
impl ChannelAdapter for CliChannel {
    async fn send(&self, message: &str) -> Result<()> {
        println!("{message}");
        Ok(())
    }

    async fn receive(&self) -> Result<String> {
        // Use spawn_blocking to avoid blocking the async runtime
        tokio::task::spawn_blocking(|| {
            let mut input = String::new();
            std::io::stdin().read_line(&mut input)?;
            Ok(input.trim().to_string())
        })
        .await
        .map_err(|e| AivyxError::Agent(format!("failed to read input: {e}")))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn cli_channel_send() {
        // Verifies send doesn't panic — actual stdout output is not captured
        let ch = CliChannel;
        ch.send("test message").await.unwrap();
    }
}
