use aivyx_agent::{AgentProfile, Persona};
use aivyx_config::AivyxDirs;
use aivyx_core::Result;

use crate::output;

pub fn create(name: &str, role: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let profile_path = dirs.agents_dir().join(format!("{name}.toml"));
    if profile_path.exists() {
        output::error(&format!("agent profile '{name}' already exists"));
        return Ok(());
    }

    let profile = match role {
        Some(r) => AgentProfile::for_role(name, r),
        None => AgentProfile::template(name, name),
    };
    profile.save(&profile_path)?;

    output::success(&format!(
        "created agent profile: {}",
        profile_path.display()
    ));
    output::kv("Role", &profile.role);
    if !profile.tool_ids.is_empty() {
        output::kv("Tools", &profile.tool_ids.join(", "));
    }
    output::kv(
        "Capabilities",
        &format!("{} scope(s) granted", profile.capabilities.len()),
    );

    Ok(())
}

pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let agents_dir = dirs.agents_dir();
    if !agents_dir.exists() {
        println!("  No agents configured.");
        return Ok(());
    }

    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&agents_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && let Some(stem) = path.file_stem()
        {
            names.push(stem.to_string_lossy().to_string());
        }
    }

    if names.is_empty() {
        println!("  No agents configured.");
        return Ok(());
    }

    names.sort();
    output::header("Configured agents");
    for name in &names {
        println!("  {name}");
    }
    println!();
    Ok(())
}

pub fn show(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let profile_path = dirs.agents_dir().join(format!("{name}.toml"));
    if !profile_path.exists() {
        output::error(&format!("agent profile '{name}' not found"));
        return Ok(());
    }

    let profile = AgentProfile::load(&profile_path)?;

    output::header(&format!("Agent: {}", profile.name));
    output::kv("Role", &profile.role);
    output::kv("Max tokens", &profile.max_tokens.to_string());

    if let Some(tier) = &profile.autonomy_tier {
        output::kv("Autonomy tier", &tier.to_string());
    } else {
        output::kv("Autonomy tier", "(default)");
    }

    if !profile.tool_ids.is_empty() {
        output::kv("Tools", &profile.tool_ids.join(", "));
    }

    if !profile.skills.is_empty() {
        output::kv("Skills", &profile.skills.join(", "));
    }

    if let Some(ref persona) = profile.persona {
        output::kv("Persona", "configured");
        output::kv("  formality", &format!("{:.1}", persona.formality));
        output::kv("  verbosity", &format!("{:.1}", persona.verbosity));
        output::kv("  warmth", &format!("{:.1}", persona.warmth));
        output::kv("  humor", &format!("{:.1}", persona.humor));
        output::kv("  confidence", &format!("{:.1}", persona.confidence));
        output::kv("  curiosity", &format!("{:.1}", persona.curiosity));
    } else {
        output::kv("Persona", "(none — using raw soul)");
    }

    println!("\n  Effective soul:");
    for line in profile.effective_soul().lines() {
        println!("    {line}");
    }
    println!();

    Ok(())
}

/// Display an agent's persona dimensions, style fields, and generated soul.
pub fn persona_show(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let profile_path = dirs.agents_dir().join(format!("{name}.toml"));
    if !profile_path.exists() {
        output::error(&format!("agent profile '{name}' not found"));
        return Ok(());
    }

    let profile = AgentProfile::load(&profile_path)?;
    output::header(&format!("Persona: {}", profile.name));

    let Some(ref persona) = profile.persona else {
        println!("  No persona configured. Using raw soul string.");
        println!();
        return Ok(());
    };

    // Dimensions
    output::kv("formality", &format!("{:.2}", persona.formality));
    output::kv("verbosity", &format!("{:.2}", persona.verbosity));
    output::kv("warmth", &format!("{:.2}", persona.warmth));
    output::kv("humor", &format!("{:.2}", persona.humor));
    output::kv("confidence", &format!("{:.2}", persona.confidence));
    output::kv("curiosity", &format!("{:.2}", persona.curiosity));

    // Voice fields
    output::kv("tone", persona.tone.as_deref().unwrap_or("(default)"));
    output::kv(
        "language_level",
        persona.language_level.as_deref().unwrap_or("(default)"),
    );
    output::kv(
        "code_style",
        persona.code_style.as_deref().unwrap_or("(default)"),
    );
    output::kv(
        "error_style",
        persona.error_style.as_deref().unwrap_or("(default)"),
    );
    output::kv(
        "greeting",
        persona.greeting.as_deref().unwrap_or("(default)"),
    );

    // Toggles
    output::kv("uses_emoji", &persona.uses_emoji.to_string());
    output::kv("uses_analogies", &persona.uses_analogies.to_string());
    output::kv("asks_followups", &persona.asks_followups.to_string());
    output::kv(
        "admits_uncertainty",
        &persona.admits_uncertainty.to_string(),
    );

    // Generated soul preview
    println!("\n  Generated soul:");
    for line in persona.generate_soul(&profile.role).lines() {
        println!("    {line}");
    }
    println!();

    Ok(())
}

/// Set persona fields on an agent, optionally starting from a preset.
#[allow(clippy::too_many_arguments)]
pub fn persona_set(
    name: &str,
    preset: Option<&str>,
    formality: Option<f32>,
    verbosity: Option<f32>,
    warmth: Option<f32>,
    humor: Option<f32>,
    confidence: Option<f32>,
    curiosity: Option<f32>,
    tone: Option<&str>,
    uses_emoji: Option<bool>,
) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let profile_path = dirs.agents_dir().join(format!("{name}.toml"));
    if !profile_path.exists() {
        output::error(&format!("agent profile '{name}' not found"));
        return Ok(());
    }

    let mut profile = AgentProfile::load(&profile_path)?;

    // Start from preset, existing persona, or default
    let mut persona = if let Some(preset_name) = preset {
        match Persona::for_role(preset_name) {
            Some(p) => p,
            None => {
                output::error(&format!(
                    "unknown preset: {preset_name}. Available: {}",
                    Persona::preset_names().join(", ")
                ));
                return Ok(());
            }
        }
    } else {
        profile.persona.clone().unwrap_or_default()
    };

    // Overlay individual fields
    if let Some(v) = formality {
        persona.formality = v;
    }
    if let Some(v) = verbosity {
        persona.verbosity = v;
    }
    if let Some(v) = warmth {
        persona.warmth = v;
    }
    if let Some(v) = humor {
        persona.humor = v;
    }
    if let Some(v) = confidence {
        persona.confidence = v;
    }
    if let Some(v) = curiosity {
        persona.curiosity = v;
    }
    if let Some(t) = tone {
        persona.tone = Some(t.to_string());
    }
    if let Some(e) = uses_emoji {
        persona.uses_emoji = e;
    }

    persona.normalize();
    profile.persona = Some(persona);
    profile.save(&profile_path)?;

    output::success(&format!("updated persona for '{name}'"));

    // Show a preview of the generated soul
    println!("\n  Generated soul:");
    for line in profile.effective_soul().lines() {
        println!("    {line}");
    }
    println!();

    Ok(())
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(aivyx_core::AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}
