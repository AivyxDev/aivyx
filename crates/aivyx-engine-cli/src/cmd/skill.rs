use std::path::{Path, PathBuf};

use aivyx_config::skill::validate_full;
use aivyx_config::{AivyxDirs, discover_skills, load_skill};
use aivyx_core::{AivyxError, Result};

use crate::output;

/// List all installed skills.
pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let skills_dir = dirs.skills_dir();
    if !skills_dir.exists() {
        println!("  No skills installed.");
        println!("  Use `aivyx skill install <path>` to install one.");
        return Ok(());
    }

    let summaries = discover_skills(&skills_dir)?;

    if summaries.is_empty() {
        println!("  No skills installed.");
        println!("  Use `aivyx skill install <path>` to install one.");
        return Ok(());
    }

    output::header("Installed skills");
    for s in &summaries {
        println!("  {:<24} {}", s.name, s.description);
    }
    println!();

    Ok(())
}

/// Show full details of a skill.
pub fn show(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let skill_path = dirs.skills_dir().join(name).join("SKILL.md");
    if !skill_path.exists() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "skill not found: {name} (expected at {})",
            skill_path.display()
        )));
    }

    let loaded = load_skill(&skill_path)?;

    output::header(&format!("Skill: {}", loaded.manifest.name));
    output::kv("Description", &loaded.manifest.description);
    if let Some(ref license) = loaded.manifest.license {
        output::kv("License", license);
    }
    if let Some(ref compat) = loaded.manifest.compatibility {
        output::kv("Compatibility", compat);
    }
    if let Some(ref tools) = loaded.manifest.allowed_tools {
        output::kv("Allowed tools", tools);
    }
    if !loaded.manifest.metadata.is_empty() {
        for (k, v) in &loaded.manifest.metadata {
            output::kv(&format!("  {k}"), v);
        }
    }
    output::kv("Path", &loaded.base_dir.to_string_lossy());

    // Show body preview (first 500 chars)
    println!();
    output::header("Body preview");
    let preview_len = loaded.body.floor_char_boundary(500.min(loaded.body.len()));
    let preview = &loaded.body[..preview_len];
    println!("{preview}");
    if loaded.body.len() > 500 {
        println!("  ... ({} more chars)", loaded.body.len() - 500);
    }
    println!();

    Ok(())
}

/// Install a skill from a local directory.
pub fn install(path: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let source = Path::new(path);
    if !source.is_dir() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "not a directory: {path}"
        )));
    }

    let skill_md = source.join("SKILL.md");
    if !skill_md.exists() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "no SKILL.md found in {path}"
        )));
    }

    // Parse and validate
    let loaded = load_skill(&skill_md)?;
    loaded.manifest.validate()?;

    // Validate directory name matches skill name
    let dir_name = source
        .file_name()
        .map(|f| f.to_string_lossy().to_string())
        .unwrap_or_default();
    if dir_name != loaded.manifest.name {
        return Err(aivyx_core::AivyxError::Config(format!(
            "directory name '{}' does not match skill name '{}' in SKILL.md",
            dir_name, loaded.manifest.name
        )));
    }

    // Copy to skills directory
    let dest = dirs.skills_dir().join(&loaded.manifest.name);
    if dest.exists() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "skill '{}' is already installed (remove first with `aivyx skill remove {}`)",
            loaded.manifest.name, loaded.manifest.name
        )));
    }

    // Ensure skills dir exists
    dirs.ensure_dirs()?;
    copy_dir_recursive(source, &dest)?;

    output::success(&format!("installed skill: {}", loaded.manifest.name));
    output::kv("Description", &loaded.manifest.description);
    output::kv("Installed to", &dest.to_string_lossy());
    println!();

    Ok(())
}

/// Remove an installed skill.
pub fn remove(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let skill_dir = dirs.skills_dir().join(name);
    if !skill_dir.exists() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "skill not found: {name}"
        )));
    }

    std::fs::remove_dir_all(&skill_dir)?;

    output::success(&format!("removed skill: {name}"));
    println!();

    Ok(())
}

/// Validate a SKILL.md file for correctness and best practices.
pub fn validate(path: &str) -> Result<()> {
    // Resolve to SKILL.md if path is a directory
    let resolved = if Path::new(path).is_dir() {
        PathBuf::from(path).join("SKILL.md")
    } else {
        PathBuf::from(path)
    };

    if !resolved.exists() {
        return Err(AivyxError::Config(format!(
            "file not found: {}",
            resolved.display()
        )));
    }

    let content = std::fs::read_to_string(&resolved)
        .map_err(|e| AivyxError::Config(format!("cannot read '{}': {e}", resolved.display())))?;

    let report = validate_full(&content);

    output::header(&format!("Validating {}", resolved.display()));

    if report.errors.is_empty() && report.warnings.is_empty() {
        output::success("no issues found");
        println!();
        return Ok(());
    }

    for err in &report.errors {
        output::error(&format!("error: {err}"));
    }
    for warn in &report.warnings {
        println!("  [warn] {warn}");
    }
    println!();

    if !report.is_ok() {
        return Err(AivyxError::Config(format!(
            "validation failed with {} error(s)",
            report.errors.len()
        )));
    }

    Ok(())
}

/// Scaffold a new skill from an interactive wizard.
pub fn create(output_dir: Option<&str>) -> Result<()> {
    use dialoguer::{Input, Select, theme::ColorfulTheme};

    let theme = ColorfulTheme::default();
    output::header("Skill Creator Wizard");

    // Step 1: Name
    let name: String = Input::with_theme(&theme)
        .with_prompt("Skill name (lowercase, hyphens, digits)")
        .validate_with(|input: &String| -> std::result::Result<(), String> {
            if input.is_empty() {
                return Err("name is required".into());
            }
            if input.len() > 64 {
                return Err("name must be 64 characters or fewer".into());
            }
            if !input
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            {
                return Err("name must be lowercase letters, digits, and hyphens only".into());
            }
            Ok(())
        })
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 2: Description
    let description: String = Input::with_theme(&theme)
        .with_prompt("Description")
        .validate_with(|input: &String| -> std::result::Result<(), String> {
            if input.is_empty() {
                return Err("description is required".into());
            }
            if input.len() > 1024 {
                return Err("description must be 1024 characters or fewer".into());
            }
            Ok(())
        })
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 3: License
    let license: String = Input::with_theme(&theme)
        .with_prompt("License (SPDX, e.g., MIT)")
        .default("MIT".into())
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 4: Compatibility
    let compatibility: String = Input::with_theme(&theme)
        .with_prompt("Compatibility notes (optional, press Enter to skip)")
        .default(String::new())
        .allow_empty(true)
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 5: Allowed tools
    let allowed_tools: String = Input::with_theme(&theme)
        .with_prompt("Allowed tools (space-separated, e.g., 'Bash(git:*) Read Write', or press Enter to skip)")
        .default(String::new())
        .allow_empty(true)
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 6: Author
    let author: String = Input::with_theme(&theme)
        .with_prompt("Author (optional)")
        .default(String::new())
        .allow_empty(true)
        .interact_text()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    // Step 7: Template style
    let templates = &["Minimal", "Standard (sections)", "Full (with examples)"];
    let template_idx = Select::with_theme(&theme)
        .with_prompt("Body template")
        .items(templates)
        .default(1)
        .interact()
        .map_err(|e| AivyxError::Other(format!("prompt error: {e}")))?;

    let content = generate_skill_content(
        &name,
        &description,
        &license,
        &compatibility,
        &allowed_tools,
        &author,
        template_idx,
    );

    // Create directory and write file
    let base = output_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let skill_dir = base.join(&name);

    if skill_dir.exists() {
        return Err(AivyxError::Config(format!(
            "directory already exists: {}",
            skill_dir.display()
        )));
    }

    std::fs::create_dir_all(&skill_dir)?;
    std::fs::write(skill_dir.join("SKILL.md"), &content)?;

    output::success(&format!("created skill at {}", skill_dir.display()));
    output::kv("Name", &name);
    output::kv("Template", templates[template_idx]);
    println!();
    println!("  Next steps:");
    println!(
        "    1. Edit {}/SKILL.md with your instructions",
        skill_dir.display()
    );
    println!(
        "    2. Run `aivyx skill validate {}` to check it",
        skill_dir.display()
    );
    println!(
        "    3. Run `aivyx skill install {}` to install it",
        skill_dir.display()
    );
    println!();

    Ok(())
}

/// Generate SKILL.md content from wizard inputs.
fn generate_skill_content(
    name: &str,
    description: &str,
    license: &str,
    compatibility: &str,
    allowed_tools: &str,
    author: &str,
    template: usize,
) -> String {
    let mut frontmatter = format!("---\nname: {name}\ndescription: {description}\n");

    if !license.is_empty() {
        frontmatter.push_str(&format!("license: {license}\n"));
    }
    if !compatibility.is_empty() {
        frontmatter.push_str(&format!("compatibility: {compatibility}\n"));
    }
    if !allowed_tools.is_empty() {
        frontmatter.push_str(&format!("allowed-tools: {allowed_tools}\n"));
    }
    if !author.is_empty() {
        frontmatter.push_str(&format!(
            "metadata:\n  author: {author}\n  version: \"1.0.0\"\n"
        ));
    }

    frontmatter.push_str("---\n");

    // Title from name
    let title: String = name
        .split('-')
        .map(|w| {
            let mut chars = w.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ");

    let body = match template {
        0 => {
            // Minimal
            format!("# {title}\n\n## Instructions\n\nAdd your instructions here.\n")
        }
        1 => {
            // Standard
            format!(
                "# {title}\n\n## Overview\n\n{description}\n\n## Workflow\n\n1. Step one\n2. Step two\n3. Step three\n\n## Configuration\n\nDescribe any configuration needed.\n"
            )
        }
        _ => {
            // Full
            format!(
                "# {title}\n\n## Overview\n\n{description}\n\n## Workflow\n\n1. Step one\n2. Step two\n3. Step three\n\n## Configuration\n\nDescribe any configuration needed.\n\n## Examples\n\n```\n# Example usage\naivyx run assistant --skill {name}\n```\n\n## Invocation\n\nThis skill is activated when the agent encounters tasks related to {title}.\n\n## Troubleshooting\n\n- **Issue**: Description of common issue\n  **Fix**: How to resolve it\n"
            )
        }
    };

    format!("{frontmatter}{body}")
}

/// Recursively copy a directory.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let entry_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());

        if entry_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
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
