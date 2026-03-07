use aivyx_agent::AgentSession;
use aivyx_config::{AivyxConfig, AivyxDirs};
use aivyx_core::Result;
use aivyx_crypto::{MasterKey, derive_team_session_key};
use aivyx_team::nonagon::all_nonagon_profiles;
use aivyx_team::{OrchestrationMode, TeamConfig, TeamMemberConfig, TeamRuntime, TeamSessionStore};

use crate::channel::CliChannel;
use crate::output;

pub fn create(name: &str, nonagon: bool) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let teams_dir = dirs.teams_dir();
    let team_path = teams_dir.join(format!("{name}.toml"));

    if team_path.exists() {
        output::error(&format!("team '{name}' already exists"));
        return Ok(());
    }

    if nonagon {
        // Generate all 9 Nonagon profiles
        let profiles = all_nonagon_profiles();
        for profile in &profiles {
            let profile_path = dirs.agents_dir().join(format!("{}.toml", profile.name));
            if !profile_path.exists() {
                profile.save(&profile_path)?;
                output::success(&format!("created agent profile: {}", profile.name));
            }
        }

        // Create team config with all 9 members
        let members: Vec<TeamMemberConfig> = profiles
            .iter()
            .map(|p| TeamMemberConfig {
                name: p.name.clone(),
                role: p.role.clone(),
            })
            .collect();

        let config = TeamConfig {
            name: name.to_string(),
            description: format!("Nonagon team '{name}' with 9 specialized agents"),
            orchestration: OrchestrationMode::LeadAgent {
                lead: "coordinator".into(),
            },
            members,
            dialogue: Default::default(),
        };

        config.save(&team_path)?;
        output::success(&format!("created Nonagon team: {name}"));
    } else {
        // Create a minimal team config
        let config = TeamConfig {
            name: name.to_string(),
            description: format!("Team '{name}'"),
            orchestration: OrchestrationMode::LeadAgent {
                lead: name.to_string(),
            },
            members: Vec::new(),
            dialogue: Default::default(),
        };
        config.save(&team_path)?;
        output::success(&format!(
            "created team: {name} (add members to {})",
            team_path.display()
        ));
    }

    Ok(())
}

pub fn list() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let teams_dir = dirs.teams_dir();
    if !teams_dir.exists() {
        println!("  No teams configured.");
        return Ok(());
    }

    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&teams_dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.extension().is_some_and(|ext| ext == "toml")
            && let Some(stem) = path.file_stem()
        {
            names.push(stem.to_string_lossy().to_string());
        }
    }

    if names.is_empty() {
        println!("  No teams configured.");
        return Ok(());
    }

    names.sort();
    output::header("Configured teams");
    for name in &names {
        println!("  {name}");
    }
    println!();
    Ok(())
}

pub fn show(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let team_path = dirs.teams_dir().join(format!("{name}.toml"));
    if !team_path.exists() {
        output::error(&format!("team '{name}' not found"));
        return Ok(());
    }

    let config = TeamConfig::load(&team_path)?;

    output::header(&format!("Team: {}", config.name));
    output::kv("Description", &config.description);

    match &config.orchestration {
        OrchestrationMode::LeadAgent { lead } => {
            output::kv("Orchestration", &format!("Lead Agent ({lead})"));
        }
    }

    if !config.members.is_empty() {
        println!("\n  Members:");
        for member in &config.members {
            println!("    {} ({})", member.name, member.role);
        }
    }
    println!();

    Ok(())
}

pub async fn run(name: &str, prompt: &str, session_id: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let config = AivyxConfig::load(dirs.config_path())?;

    // Derive team session key + open store for persistence
    let ts_key = derive_team_session_key(&master_key);
    let ts_dir = dirs.team_sessions_dir();
    std::fs::create_dir_all(&ts_dir)?;
    let store = TeamSessionStore::open(ts_dir.join("team-sessions.db"))?;

    let session = AgentSession::new(dirs, config, master_key);
    let dirs = AivyxDirs::from_default()?;
    let runtime = TeamRuntime::load(name, &dirs, session)?;

    // Restore from session if provided
    if let Some(sid) = session_id {
        if let Some(persisted) = store.load(name, sid, &ts_key)? {
            output::header(&format!("Resuming team: {name} (session: {sid})"));
            output::kv(
                "Prior work items",
                &persisted.completed_work.len().to_string(),
            );
            println!();
        } else {
            output::header(&format!(
                "Running team: {name} (new session, id not found: {sid})"
            ));
            println!();
        }
    } else {
        output::header(&format!("Running team: {name}"));
        println!();
    }

    let (token_tx, mut token_rx) = tokio::sync::mpsc::channel::<String>(64);

    let print_handle = tokio::spawn(async move {
        use std::io::Write;
        while let Some(token) = token_rx.recv().await {
            print!("{token}");
            std::io::stdout().flush().ok();
        }
        println!();
    });

    let cli_channel = CliChannel;
    let result = runtime
        .run_stream(prompt, Some(&cli_channel), token_tx)
        .await?;
    let _ = print_handle.await;

    // Save session after run
    let final_session_id = session_id
        .map(|s| s.to_string())
        .unwrap_or_else(|| uuid::Uuid::new_v4().to_string());

    let persisted = aivyx_team::PersistedTeamSession {
        session_id: final_session_id.clone(),
        team_name: name.to_string(),
        lead_conversation: Vec::new(), // Lead conversation is internal to runtime
        specialist_conversations: std::collections::HashMap::new(),
        completed_work: vec![format!(
            "Goal: {prompt}\nResult: {} chars output",
            result.len()
        )],
        created_at: chrono::Utc::now().to_rfc3339(),
        updated_at: chrono::Utc::now().to_rfc3339(),
    };
    store.save(&persisted, &ts_key)?;

    println!();
    output::success(&format!("Team run completed ({} chars)", result.len()));
    output::kv("Session ID", &final_session_id);
    println!("  Resume with: aivyx team run {name} \"...\" --session {final_session_id}");
    Ok(())
}

/// List saved team sessions.
pub fn session_list(name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let master_key = unlock_master_key(&dirs)?;
    let ts_key = derive_team_session_key(&master_key);
    let ts_dir = dirs.team_sessions_dir();

    if !ts_dir.exists() {
        println!("  No team sessions saved.");
        return Ok(());
    }

    let store = TeamSessionStore::open(ts_dir.join("team-sessions.db"))?;
    let sessions = store.list(name, &ts_key)?;

    if sessions.is_empty() {
        println!("  No sessions for team '{name}'.");
        return Ok(());
    }

    output::header(&format!("Sessions for team: {name}"));
    for meta in &sessions {
        output::kv("Session", &meta.session_id);
        output::kv("  Lead messages", &meta.lead_message_count.to_string());
        output::kv("  Specialists", &meta.specialist_count.to_string());
        output::kv("  Completed work", &meta.completed_work_count.to_string());
        output::kv("  Updated", &meta.updated_at);
        println!();
    }

    Ok(())
}

/// Delete a saved team session.
pub fn session_delete(name: &str, session_id: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let ts_dir = dirs.team_sessions_dir();
    if !ts_dir.exists() {
        output::error("no team sessions directory found");
        return Ok(());
    }

    let store = TeamSessionStore::open(ts_dir.join("team-sessions.db"))?;
    store.delete(name, session_id)?;
    output::success(&format!("deleted session '{session_id}' for team '{name}'"));
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

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
