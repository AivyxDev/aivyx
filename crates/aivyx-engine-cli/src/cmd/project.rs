use std::path::Path;

use aivyx_config::{AivyxConfig, AivyxDirs, ProjectConfig};
use aivyx_core::Result;

use crate::output;

/// Register a new project directory.
pub fn add(path: &str, name: Option<&str>, language: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    // Resolve to absolute path
    let abs_path = std::fs::canonicalize(path)
        .map_err(|e| aivyx_core::AivyxError::Config(format!("invalid path '{path}': {e}")))?;

    if !abs_path.is_dir() {
        return Err(aivyx_core::AivyxError::Config(format!(
            "not a directory: {}",
            abs_path.display()
        )));
    }

    // Default name = last path component, sanitized to slug
    let project_name = match name {
        Some(n) => n.to_string(),
        None => abs_path
            .file_name()
            .map(|f| f.to_string_lossy().to_string())
            .unwrap_or_else(|| "project".to_string()),
    };

    let mut config = AivyxConfig::load(dirs.config_path())?;

    let mut project = ProjectConfig::new(&project_name, &abs_path);

    // Set language (auto-detect if not provided)
    project.language = language
        .map(|l| l.to_string())
        .or_else(|| detect_language(&abs_path));

    // Auto-read README for description
    project.description = read_readme_summary(&abs_path);

    config.add_project(project.clone())?;
    config.save(dirs.config_path())?;

    // Audit log
    if let Ok(master_key) = unlock_master_key(&dirs) {
        let audit_key = aivyx_crypto::derive_audit_key(&master_key);
        let audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &audit_key);
        let _ = audit_log.append(aivyx_audit::AuditEvent::ProjectRegistered {
            project_name: project.name.clone(),
            project_path: abs_path.to_string_lossy().to_string(),
        });
    }

    output::success(&format!("registered project: {}", project.name));
    output::kv("Path", &abs_path.to_string_lossy());
    if let Some(ref lang) = project.language {
        output::kv("Language", lang);
    }
    if let Some(ref desc) = project.description {
        output::kv("Description", desc);
    }
    println!();

    Ok(())
}

/// List all registered projects.
pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;

    if config.projects.is_empty() {
        println!("  No projects registered.");
        println!("  Use `aivyx project add <path>` to register one.");
        return Ok(());
    }

    output::header("Registered projects");
    for p in &config.projects {
        let lang = p.language.as_deref().unwrap_or("-");
        println!("  {:<20} {:<12} {}", p.name, lang, p.path.display());
    }
    println!();

    Ok(())
}

/// Show details of a single project.
pub fn show(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let config = AivyxConfig::load(dirs.config_path())?;

    let project = config
        .find_project(name)
        .ok_or_else(|| aivyx_core::AivyxError::Config(format!("project not found: {name}")))?;

    output::header(&format!("Project: {}", project.name));
    output::kv("Path", &project.path.to_string_lossy());
    output::kv(
        "Language",
        project.language.as_deref().unwrap_or("(not set)"),
    );
    output::kv(
        "Description",
        project.description.as_deref().unwrap_or("(none)"),
    );
    output::kv("Tag", &project.project_tag());
    output::kv("Registered", &project.registered_at.to_rfc3339());
    if !project.tags.is_empty() {
        output::kv("Tags", &project.tags.join(", "));
    }
    println!();

    Ok(())
}

/// Remove a registered project.
pub fn remove(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let mut config = AivyxConfig::load(dirs.config_path())?;
    let removed = config.remove_project(name)?;
    config.save(dirs.config_path())?;

    // Audit log
    if let Ok(master_key) = unlock_master_key(&dirs) {
        let audit_key = aivyx_crypto::derive_audit_key(&master_key);
        let audit_log = aivyx_audit::AuditLog::new(dirs.audit_path(), &audit_key);
        let _ = audit_log.append(aivyx_audit::AuditEvent::ProjectRemoved {
            project_name: removed.name.clone(),
        });
    }

    output::success(&format!("removed project: {}", removed.name));
    println!();

    Ok(())
}

/// Auto-detect the primary language from file extensions in the project root.
fn detect_language(path: &Path) -> Option<String> {
    let entries = std::fs::read_dir(path).ok()?;
    let mut has_rs = false;
    let mut has_py = false;
    let mut has_ts = false;
    let mut has_js = false;
    let mut has_go = false;

    for entry in entries.filter_map(|e| e.ok()) {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Check for config files that indicate the language
        match name_str.as_ref() {
            "Cargo.toml" => return Some("Rust".into()),
            "pyproject.toml" | "setup.py" | "requirements.txt" => return Some("Python".into()),
            "tsconfig.json" => return Some("TypeScript".into()),
            "go.mod" => return Some("Go".into()),
            "package.json" => has_js = true,
            _ => {}
        }

        // Check file extensions
        if name_str.ends_with(".rs") {
            has_rs = true;
        } else if name_str.ends_with(".py") {
            has_py = true;
        } else if name_str.ends_with(".ts") || name_str.ends_with(".tsx") {
            has_ts = true;
        } else if name_str.ends_with(".js") || name_str.ends_with(".jsx") {
            has_js = true;
        } else if name_str.ends_with(".go") {
            has_go = true;
        }
    }

    if has_rs {
        Some("Rust".into())
    } else if has_ts {
        Some("TypeScript".into())
    } else if has_py {
        Some("Python".into())
    } else if has_go {
        Some("Go".into())
    } else if has_js {
        Some("JavaScript".into())
    } else {
        None
    }
}

/// Read the first ~500 characters of README.md as a description.
fn read_readme_summary(path: &Path) -> Option<String> {
    let readme_path = path.join("README.md");
    if !readme_path.exists() {
        return None;
    }

    let content = std::fs::read_to_string(&readme_path).ok()?;
    if content.is_empty() {
        return None;
    }

    // Take the first 500 chars, truncate at a word boundary
    let max_len = 500;
    if content.len() <= max_len {
        Some(content.trim().to_string())
    } else {
        let boundary = content.floor_char_boundary(max_len);
        let truncated = &content[..boundary];
        // Try to break at a newline or space
        if let Some(pos) = truncated.rfind('\n') {
            Some(truncated[..pos].trim().to_string())
        } else if let Some(pos) = truncated.rfind(' ') {
            Some(format!("{}...", truncated[..pos].trim()))
        } else {
            Some(format!("{truncated}..."))
        }
    }
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

fn unlock_master_key(dirs: &AivyxDirs) -> Result<aivyx_crypto::MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
