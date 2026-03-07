//! Genesis Wizard — guided first-run setup for aivyx.
//!
//! Replaces the minimal `aivyx init` with a rich 7-step ceremony that walks new
//! users through identity, provider selection, passphrase creation, persona
//! customization, and optional project registration. All choices are collected
//! into a [`GenesisState`] and committed atomically at the end.
//!
//! The `--yes` flag runs non-interactively with sensible defaults.

use std::path::{Path, PathBuf};

use aivyx_agent::{AgentProfile, Persona};
use aivyx_audit::{AuditEvent, AuditLog};
use aivyx_config::{AivyxConfig, AivyxDirs, EmbeddingConfig, ProjectConfig, ProviderConfig};
use aivyx_core::Result;
use aivyx_crypto::{EncryptedStore, MasterKey, derive_audit_key};
use chrono::Utc;

use crate::output;

// ── Wizard state ────────────────────────────────────────────────────

/// Collected answers from all wizard steps. Written atomically in step 7.
struct GenesisState {
    /// User's display name (optional).
    user_name: Option<String>,
    /// IANA timezone string (auto-detected, overridable).
    timezone: String,
    /// Chosen provider configuration (api_key_ref will point into store).
    provider: ProviderConfig,
    /// Raw API key to store encrypted (None for Ollama).
    api_key: Option<String>,
    /// The name under which the API key is stored.
    api_key_ref: String,
    /// User-chosen passphrase.
    passphrase: String,
    /// Which persona preset for the default agent.
    persona_role: String,
    /// The persona (possibly tuned).
    persona: Persona,
    /// Registered projects (may be empty).
    projects: Vec<ProjectConfig>,
}

// ── Public entry point ──────────────────────────────────────────────

/// Run the Genesis wizard.
///
/// When `non_interactive` is true, sensible defaults are used and only the
/// passphrase and (for cloud providers) API key are prompted via
/// environment variables or `rpassword`.
pub async fn run(non_interactive: bool) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;

    if dirs.is_initialized() {
        output::error("aivyx is already initialized.");
        println!("  Data directory: {}", dirs.root().display());
        println!(
            "  To reconfigure, edit {} or delete the data directory.",
            dirs.config_path().display()
        );
        return Ok(());
    }

    let state = if non_interactive {
        run_non_interactive()?
    } else {
        run_interactive()?
    };

    commit(state, &dirs)?;

    Ok(())
}

// ── Non-interactive mode ────────────────────────────────────────────

fn run_non_interactive() -> Result<GenesisState> {
    output::header("aivyx genesis (non-interactive)");

    // User name (optional)
    let user_name = std::env::var("AIVYX_USER_NAME")
        .ok()
        .filter(|s| !s.is_empty());

    // Timezone
    let tz = std::env::var("AIVYX_TIMEZONE")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(detect_timezone);

    // Provider selection
    let provider_str = std::env::var("AIVYX_PROVIDER")
        .ok()
        .unwrap_or_else(|| "anthropic".into());

    let (provider, api_key, api_key_ref) = match provider_str.to_lowercase().as_str() {
        "ollama" => {
            let base_url = std::env::var("AIVYX_OLLAMA_URL")
                .ok()
                .unwrap_or_else(|| "http://localhost:11434".into());
            let model = std::env::var("AIVYX_OLLAMA_MODEL")
                .ok()
                .unwrap_or_else(|| "llama3".into());
            (
                ProviderConfig::Ollama { base_url, model },
                None,
                String::new(),
            )
        }
        "openai" => {
            let key = read_env_or_prompt_passphrase("AIVYX_API_KEY", "  Enter OpenAI API key: ")?;
            let ref_name = "openai_api_key".to_string();
            (
                ProviderConfig::OpenAI {
                    api_key_ref: ref_name.clone(),
                    model: "gpt-4o".into(),
                },
                Some(key),
                ref_name,
            )
        }
        "google" => {
            let key =
                read_env_or_prompt_passphrase("AIVYX_API_KEY", "  Enter Google AI API key: ")?;
            let ref_name = "google_api_key".to_string();
            (
                ProviderConfig::OpenAI {
                    api_key_ref: ref_name.clone(),
                    model: "gemini-2.5-pro".into(),
                },
                Some(key),
                ref_name,
            )
        }
        _ => {
            // Default to Claude/Anthropic
            let key = read_env_or_prompt_passphrase("AIVYX_API_KEY", "  Enter API key: ")?;
            let ref_name = "claude_api_key".to_string();
            (
                ProviderConfig::Claude {
                    api_key_ref: ref_name.clone(),
                    model: "claude-sonnet-4-20250514".into(),
                },
                Some(key),
                ref_name,
            )
        }
    };

    // Passphrase (from env or stdin via rpassword)
    let passphrase = read_env_or_prompt_passphrase("AIVYX_PASSPHRASE", "  Enter passphrase: ")?;
    if passphrase.is_empty() {
        return Err(aivyx_core::AivyxError::Other(
            "passphrase cannot be empty".into(),
        ));
    }

    // Persona preset
    let persona_role = std::env::var("AIVYX_PERSONA")
        .ok()
        .unwrap_or_else(|| "assistant".into());

    let mut persona = Persona::for_role(&persona_role).unwrap_or_default();

    // Override individual dimensions from env vars
    if let Ok(v) = std::env::var("AIVYX_PERSONA_FORMALITY")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.formality = f;
    }
    if let Ok(v) = std::env::var("AIVYX_PERSONA_VERBOSITY")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.verbosity = f;
    }
    if let Ok(v) = std::env::var("AIVYX_PERSONA_WARMTH")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.warmth = f;
    }
    if let Ok(v) = std::env::var("AIVYX_PERSONA_HUMOR")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.humor = f;
    }
    if let Ok(v) = std::env::var("AIVYX_PERSONA_CONFIDENCE")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.confidence = f;
    }
    if let Ok(v) = std::env::var("AIVYX_PERSONA_CURIOSITY")
        && let Ok(f) = v.parse::<f32>()
    {
        persona.curiosity = f;
    }
    persona.normalize();

    Ok(GenesisState {
        user_name,
        timezone: tz,
        provider,
        api_key,
        api_key_ref,
        passphrase,
        persona_role,
        persona,
        projects: Vec::new(),
    })
}

// ── Interactive mode (7 steps) ──────────────────────────────────────

fn run_interactive() -> Result<GenesisState> {
    use dialoguer::{Confirm, Input, Password, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();

    // ── Step 1: Welcome ─────────────────────────────────────────────
    println!();
    println!("       *");
    println!("      ***");
    println!("     *****");
    println!("      |||");
    println!("      |||");
    println!();
    println!("    a i v y x");
    println!("    ─────────");
    println!("    Secure, privacy-first AI agent framework");
    println!();
    println!("    This wizard will guide you through initial setup:");
    println!("      1. Identity       — who you are");
    println!("      2. Provider       — your LLM backend");
    println!("      3. Passphrase     — encrypt your data");
    println!("      4. Persona        — your agent's personality");
    println!("      5. Projects       — register workspaces");
    println!();

    let ready = Confirm::with_theme(&theme)
        .with_prompt("Ready to begin?")
        .default(true)
        .interact()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    if !ready {
        return Err(aivyx_core::AivyxError::Other("setup cancelled".into()));
    }

    // ── Step 2: Identity ────────────────────────────────────────────
    output::header("Step 1: Identity");

    let user_name: String = Input::with_theme(&theme)
        .with_prompt("Your name (optional)")
        .default(String::new())
        .allow_empty(true)
        .interact_text()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    let user_name = if user_name.trim().is_empty() {
        None
    } else {
        Some(user_name.trim().to_string())
    };

    let detected_tz = detect_timezone();
    println!("  Detected timezone: {detected_tz}");

    let timezone: String = Input::with_theme(&theme)
        .with_prompt("Timezone")
        .default(detected_tz)
        .interact_text()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    // ── Step 3: Provider ────────────────────────────────────────────
    output::header("Step 2: LLM Provider");

    let provider_items = &[
        "Claude (recommended)",
        "OpenAI",
        "Ollama (local, no API key)",
    ];
    let provider_idx = Select::with_theme(&theme)
        .with_prompt("Choose your LLM provider")
        .items(provider_items)
        .default(0)
        .interact()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    let (provider, api_key, api_key_ref) = match provider_idx {
        0 => {
            // Claude
            let key = Password::with_theme(&theme)
                .with_prompt("Claude API key")
                .interact()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            let model: String = Input::with_theme(&theme)
                .with_prompt("Model")
                .default("claude-sonnet-4-20250514".into())
                .interact_text()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            let ref_name = "claude_api_key".to_string();
            (
                ProviderConfig::Claude {
                    api_key_ref: ref_name.clone(),
                    model,
                },
                Some(key),
                ref_name,
            )
        }
        1 => {
            // OpenAI
            let key = Password::with_theme(&theme)
                .with_prompt("OpenAI API key")
                .interact()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            let model: String = Input::with_theme(&theme)
                .with_prompt("Model")
                .default("gpt-4o".into())
                .interact_text()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            let ref_name = "openai_api_key".to_string();
            (
                ProviderConfig::OpenAI {
                    api_key_ref: ref_name.clone(),
                    model,
                },
                Some(key),
                ref_name,
            )
        }
        _ => {
            // Ollama
            let base_url: String = Input::with_theme(&theme)
                .with_prompt("Ollama base URL")
                .default("http://localhost:11434".into())
                .interact_text()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            let model: String = Input::with_theme(&theme)
                .with_prompt("Model")
                .default("llama3".into())
                .interact_text()
                .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;
            (
                ProviderConfig::Ollama { base_url, model },
                None,
                String::new(),
            )
        }
    };

    // ── Step 4: Passphrase ──────────────────────────────────────────
    output::header("Step 3: Passphrase");
    println!("  Your passphrase encrypts all secrets and memory data.");
    println!("  Choose something memorable — there is no recovery mechanism.");
    println!();

    let passphrase = Password::with_theme(&theme)
        .with_prompt("Choose a passphrase")
        .with_confirmation("Confirm passphrase", "Passphrases don't match")
        .interact()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    if passphrase.is_empty() {
        return Err(aivyx_core::AivyxError::Other(
            "passphrase cannot be empty".into(),
        ));
    }

    let strength = passphrase_strength(&passphrase);
    println!("  Strength: {strength}");

    // ── Step 5: Persona ─────────────────────────────────────────────
    output::header("Step 4: Persona");
    println!("  Choose a personality preset for your default agent.");
    println!();

    let persona_choices = &[
        "assistant — balanced, helpful all-rounder",
        "coder — precise, technical, concise",
        "researcher — thorough, analytical, cautious",
        "writer — expressive, clear, detail-oriented",
        "ops — direct, systematic, monitoring-focused",
    ];
    let persona_keys = &["assistant", "coder", "researcher", "writer", "ops"];

    let persona_idx = Select::with_theme(&theme)
        .with_prompt("Default agent persona")
        .items(persona_choices)
        .default(0)
        .interact()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    let persona_role = persona_keys[persona_idx].to_string();
    let mut persona = Persona::for_role(&persona_role).unwrap_or_default();

    let tune = Confirm::with_theme(&theme)
        .with_prompt("Tune persona dimensions?")
        .default(false)
        .interact()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    if tune {
        persona = tune_persona(persona, &theme)?;
    }

    // ── Step 6: Projects ────────────────────────────────────────────
    output::header("Step 5: Projects (optional)");

    let mut projects = Vec::new();
    loop {
        let add = Confirm::with_theme(&theme)
            .with_prompt("Register a project directory?")
            .default(projects.is_empty()) // default yes first time
            .interact()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

        if !add {
            break;
        }

        let path_str: String = Input::with_theme(&theme)
            .with_prompt("Project path")
            .interact_text()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

        let path = PathBuf::from(&path_str);
        if !path.is_dir() {
            println!("  Warning: {path_str} is not a directory, skipping.");
            continue;
        }

        let dir_name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".into());

        let name: String = Input::with_theme(&theme)
            .with_prompt("Project name")
            .default(dir_name)
            .interact_text()
            .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

        let mut project = ProjectConfig::new(name, &path);
        project.language = detect_language(&path);

        if let Some(ref lang) = project.language {
            println!("  Detected language: {lang}");
        }

        projects.push(project);
    }

    Ok(GenesisState {
        user_name,
        timezone,
        provider,
        api_key,
        api_key_ref,
        passphrase,
        persona_role,
        persona,
        projects,
    })
}

// ── Step 7: Commit ──────────────────────────────────────────────────

/// Atomically write all configuration to disk.
fn commit(state: GenesisState, dirs: &AivyxDirs) -> Result<()> {
    output::header("Lighting the candle");

    // 1. Create directory structure
    dirs.ensure_dirs()?;
    output::success(&format!("Created {}", dirs.root().display()));

    // 2. Generate and encrypt master key
    let master_key = MasterKey::generate();
    let envelope = master_key.encrypt_to_envelope(state.passphrase.as_bytes())?;
    let envelope_json =
        serde_json::to_string_pretty(&envelope).map_err(aivyx_core::AivyxError::Serialization)?;
    std::fs::write(dirs.master_key_path(), envelope_json)?;
    output::success("Generated and encrypted master key");

    // 3. Store API key in encrypted store (if cloud provider)
    if let Some(ref api_key) = state.api_key
        && !state.api_key_ref.is_empty()
    {
        let store = EncryptedStore::open(dirs.store_path())?;
        store.put(&state.api_key_ref, api_key.as_bytes(), &master_key)?;
        output::success(&format!("Stored API key as '{}'", state.api_key_ref));
    }

    // 4. Build and write config
    let config = AivyxConfig {
        provider: state.provider,
        embedding: Some(EmbeddingConfig::default()),
        projects: state.projects,
        ..Default::default()
    };
    config.save(dirs.config_path())?;
    output::success("Wrote config.toml");

    // 5. Create default agent profile with persona
    let mut profile = AgentProfile::for_role("aivyx", &state.persona_role);
    profile.persona = Some(state.persona);
    let agent_path = dirs.agents_dir().join("aivyx.toml");
    profile.save(&agent_path)?;
    output::success("Created agent profile 'aivyx'");

    // 6. Initialize audit log
    let audit_key = derive_audit_key(&master_key);
    let audit_log = AuditLog::new(dirs.audit_path(), &audit_key);
    audit_log.append(AuditEvent::SystemInit {
        timestamp: Utc::now(),
    })?;
    output::success("Initialized audit log");

    // 7. Print ceremony
    println!();
    println!("       *");
    println!("      ***");
    println!("     *****");
    println!("      |||");
    println!("      |||");
    println!();
    println!("    Your aivyx candle is lit.");
    println!();

    // Summary
    output::header("Summary");
    output::kv("Data directory", &dirs.root().display().to_string());
    output::kv("Config", &dirs.config_path().display().to_string());
    output::kv("Agent", "aivyx");
    output::kv("Persona", &state.persona_role);
    if let Some(ref name) = state.user_name {
        output::kv("User", name);
    }
    output::kv("Timezone", &state.timezone);

    println!();
    println!("  Run `aivyx chat aivyx` to begin.");
    println!();

    Ok(())
}

// ── Helpers ─────────────────────────────────────────────────────────

/// Detect the local timezone offset as a UTC±HH:MM string.
fn detect_timezone() -> String {
    let offset = chrono::Local::now().offset().to_string();
    if offset == "+00:00" {
        "UTC".into()
    } else {
        format!("UTC{offset}")
    }
}

/// Classify passphrase strength.
fn passphrase_strength(passphrase: &str) -> &'static str {
    let len = passphrase.len();
    let has_upper = passphrase.chars().any(|c| c.is_uppercase());
    let has_lower = passphrase.chars().any(|c| c.is_lowercase());
    let has_digit = passphrase.chars().any(|c| c.is_ascii_digit());
    let has_symbol = passphrase.chars().any(|c| !c.is_alphanumeric());

    let variety = [has_upper, has_lower, has_digit, has_symbol]
        .iter()
        .filter(|&&b| b)
        .count();

    if len >= 12 && variety >= 3 {
        "strong"
    } else if len >= 8 && variety >= 2 {
        "moderate"
    } else {
        "weak"
    }
}

/// Read a value from an environment variable, or prompt with rpassword.
fn read_env_or_prompt_passphrase(env_var: &str, prompt: &str) -> Result<String> {
    if let Ok(val) = std::env::var(env_var)
        && !val.is_empty()
    {
        return Ok(val);
    }
    rpassword::prompt_password(prompt)
        .map_err(|e| aivyx_core::AivyxError::Other(format!("failed to read input: {e}")))
}

/// Tune individual persona dimensions interactively.
fn tune_persona(mut persona: Persona, theme: &dialoguer::theme::ColorfulTheme) -> Result<Persona> {
    println!("  Adjust dimensions (0.0 to 1.0). Press Enter to keep current value.");
    println!();

    persona.formality = prompt_dimension(theme, "Formality (casual..formal)", persona.formality)?;
    persona.verbosity = prompt_dimension(theme, "Verbosity (terse..detailed)", persona.verbosity)?;
    persona.warmth = prompt_dimension(theme, "Warmth (neutral..warm)", persona.warmth)?;
    persona.humor = prompt_dimension(theme, "Humor (serious..playful)", persona.humor)?;
    persona.confidence =
        prompt_dimension(theme, "Confidence (hedging..assertive)", persona.confidence)?;
    persona.curiosity =
        prompt_dimension(theme, "Curiosity (focused..exploratory)", persona.curiosity)?;

    persona.normalize();
    Ok(persona)
}

/// Prompt for a single f32 dimension with a default.
fn prompt_dimension(
    theme: &dialoguer::theme::ColorfulTheme,
    label: &str,
    current: f32,
) -> Result<f32> {
    let val: String = dialoguer::Input::with_theme(theme)
        .with_prompt(format!("{label} [{current:.1}]"))
        .default(format!("{current:.1}"))
        .allow_empty(true)
        .interact_text()
        .map_err(|e| aivyx_core::AivyxError::Other(format!("input error: {e}")))?;

    val.trim()
        .parse::<f32>()
        .map_err(|_| aivyx_core::AivyxError::Other(format!("invalid number: {val}")))
}

/// Simple language detection from project root files.
fn detect_language(path: &Path) -> Option<String> {
    let entries = std::fs::read_dir(path).ok()?;
    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        match name_str.as_ref() {
            "Cargo.toml" => return Some("Rust".into()),
            "pyproject.toml" | "setup.py" | "requirements.txt" => return Some("Python".into()),
            "tsconfig.json" => return Some("TypeScript".into()),
            "go.mod" => return Some("Go".into()),
            "package.json" => return Some("JavaScript".into()),
            _ => {}
        }
    }
    None
}

// ── Tests ───────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn passphrase_strength_weak() {
        assert_eq!(passphrase_strength("abc"), "weak");
        assert_eq!(passphrase_strength("short"), "weak");
    }

    #[test]
    fn passphrase_strength_moderate() {
        assert_eq!(passphrase_strength("Password1"), "moderate");
        assert_eq!(passphrase_strength("abcdefgh12"), "moderate");
    }

    #[test]
    fn passphrase_strength_strong() {
        assert_eq!(passphrase_strength("MyPassw0rd!abc"), "strong");
        assert_eq!(passphrase_strength("C0mpl3x!Pass"), "strong");
    }

    #[test]
    fn detect_timezone_returns_string() {
        let tz = detect_timezone();
        assert!(!tz.is_empty());
        // Should start with "UTC"
        assert!(tz.starts_with("UTC"));
    }

    #[test]
    fn detect_language_rust() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        assert_eq!(detect_language(dir.path()), Some("Rust".into()));
    }

    #[test]
    fn detect_language_python() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("pyproject.toml"), "").unwrap();
        assert_eq!(detect_language(dir.path()), Some("Python".into()));
    }

    #[test]
    fn detect_language_typescript() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("tsconfig.json"), "").unwrap();
        assert_eq!(detect_language(dir.path()), Some("TypeScript".into()));
    }

    #[test]
    fn detect_language_none() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(detect_language(dir.path()), None);
    }

    #[test]
    fn genesis_state_defaults_produce_valid_config() {
        let state = GenesisState {
            user_name: None,
            timezone: "UTC".into(),
            provider: ProviderConfig::default(),
            api_key: Some("sk-test".into()),
            api_key_ref: "claude_api_key".into(),
            passphrase: "testpass123".into(),
            persona_role: "assistant".into(),
            persona: Persona::for_role("assistant").unwrap(),
            projects: Vec::new(),
        };

        // Verify the state can produce a valid AivyxConfig
        let config = AivyxConfig {
            provider: state.provider,
            embedding: Some(EmbeddingConfig::default()),
            projects: state.projects,
            ..Default::default()
        };
        assert!(matches!(config.provider, ProviderConfig::Claude { .. }));
    }

    #[test]
    fn provider_config_from_each_selection() {
        // Claude
        let claude = ProviderConfig::Claude {
            api_key_ref: "claude_api_key".into(),
            model: "claude-sonnet-4-20250514".into(),
        };
        assert!(matches!(claude, ProviderConfig::Claude { .. }));

        // OpenAI
        let openai = ProviderConfig::OpenAI {
            api_key_ref: "openai_api_key".into(),
            model: "gpt-4o".into(),
        };
        assert!(matches!(openai, ProviderConfig::OpenAI { .. }));

        // Ollama
        let ollama = ProviderConfig::Ollama {
            base_url: "http://localhost:11434".into(),
            model: "llama3".into(),
        };
        assert!(matches!(ollama, ProviderConfig::Ollama { .. }));
    }

    #[test]
    fn commit_creates_full_setup() {
        let dir = tempfile::tempdir().unwrap();
        let dirs = AivyxDirs::new(dir.path().to_path_buf());

        let state = GenesisState {
            user_name: Some("Test User".into()),
            timezone: "UTC+02:00".into(),
            provider: ProviderConfig::Ollama {
                base_url: "http://localhost:11434".into(),
                model: "llama3".into(),
            },
            api_key: None,
            api_key_ref: String::new(),
            passphrase: "test-passphrase-123".into(),
            persona_role: "coder".into(),
            persona: Persona::for_role("coder").unwrap(),
            projects: Vec::new(),
        };

        commit(state, &dirs).unwrap();

        // Verify files were created
        assert!(dirs.config_path().exists());
        assert!(dirs.master_key_path().exists());
        assert!(dirs.audit_path().exists());
        assert!(dirs.agents_dir().join("aivyx.toml").exists());

        // Verify config
        let config = AivyxConfig::load(dirs.config_path()).unwrap();
        assert!(matches!(config.provider, ProviderConfig::Ollama { .. }));

        // Verify agent profile has persona
        let profile = AgentProfile::load(dirs.agents_dir().join("aivyx.toml")).unwrap();
        assert_eq!(profile.name, "aivyx");
        assert!(profile.persona.is_some());
    }
}
