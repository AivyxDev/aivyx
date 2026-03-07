use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::Result;

use crate::output;

pub fn run() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;

    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let config = AivyxConfig::load(dirs.config_path())?;

    output::header("aivyx status");

    // Provider info.
    let provider_desc = match &config.provider {
        aivyx_config::ProviderConfig::Claude { model, .. } => format!("Claude ({model})"),
        aivyx_config::ProviderConfig::OpenAI { model, .. } => format!("OpenAI ({model})"),
        aivyx_config::ProviderConfig::Ollama { model, base_url } => {
            format!("Ollama ({model} @ {base_url})")
        }
    };
    output::kv("Provider", &provider_desc);
    output::kv("Autonomy tier", &config.autonomy.default_tier.to_string());
    output::kv(
        "Max tool calls/min",
        &config.autonomy.max_tool_calls_per_minute.to_string(),
    );
    output::kv(
        "Max cost/session",
        &format!("${:.2}", config.autonomy.max_cost_per_session_usd),
    );
    output::kv(
        "Destructive approval",
        &config.autonomy.require_approval_for_destructive.to_string(),
    );
    output::kv("Data directory", &dirs.root().display().to_string());

    // Audit stats.
    if dirs.audit_path().exists() {
        // We can't read the audit log without the master key / audit HMAC key,
        // but we can count lines as a rough entry count.
        let content = std::fs::read_to_string(dirs.audit_path())?;
        let entry_count = content.lines().filter(|l| !l.trim().is_empty()).count();
        output::kv("Audit entries", &entry_count.to_string());
    } else {
        output::kv("Audit entries", "0");
    }

    println!();
    Ok(())
}
