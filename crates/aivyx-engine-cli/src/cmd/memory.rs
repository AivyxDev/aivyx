use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, MemoryId, Result};
use aivyx_crypto::{MasterKey, derive_memory_key};
use aivyx_memory::MemoryStore;

use crate::output;

pub fn list(kind: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_memory_key(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let ids = store.list_memories()?;

    if ids.is_empty() {
        println!("  No stored memories.");
        return Ok(());
    }

    output::header("Stored Memories");
    println!();

    for id in &ids {
        if let Some(entry) = store.load_memory(id, &master_key)? {
            // Optional kind filter
            if let Some(k) = kind {
                let entry_kind = format!("{:?}", entry.kind);
                if !entry_kind.eq_ignore_ascii_case(k) {
                    continue;
                }
            }

            output::kv("  ID", &id.to_string());
            output::kv("  Kind", &format!("{:?}", entry.kind));
            output::kv("  Content", &truncate(&entry.content, 80));
            output::kv("  Tags", &entry.tags.join(", "));
            output::kv("  Accessed", &entry.access_count.to_string());
            output::kv(
                "  Created",
                &entry.created_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            );
            println!();
        }
    }

    Ok(())
}

pub fn stats() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let memory_count = store.list_memories()?.len();
    let triple_count = store.list_triples()?.len();

    output::header("Memory Stats");
    output::kv("  Memories", &memory_count.to_string());
    output::kv("  Triples", &triple_count.to_string());

    Ok(())
}

pub fn delete(id_str: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;

    let memory_id: MemoryId = id_str
        .parse()
        .map_err(|_| AivyxError::Config(format!("invalid memory ID: {id_str}")))?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    store.delete_memory(&memory_id)?;
    store.delete_embedding(&memory_id)?;

    output::success(&format!("Deleted memory {id_str}"));
    Ok(())
}

pub fn triples(subject: Option<&str>, predicate: Option<&str>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_memory_key(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let ids = store.list_triples()?;

    if ids.is_empty() {
        println!("  No stored triples.");
        return Ok(());
    }

    output::header("Knowledge Triples");
    println!();

    for id in &ids {
        if let Some(triple) = store.load_triple(id, &master_key)? {
            if let Some(s) = subject
                && triple.subject != s
            {
                continue;
            }
            if let Some(p) = predicate
                && triple.predicate != p
            {
                continue;
            }

            println!(
                "  {} {} {} (confidence: {:.0}%)",
                triple.subject,
                triple.predicate,
                triple.object,
                triple.confidence * 100.0
            );
        }
    }

    Ok(())
}

pub fn profile_show() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_memory_key(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let profile = store.load_profile(&master_key)?;

    match profile {
        Some(p) if !p.is_empty() => {
            output::header("User Profile");
            println!();
            if let Some(ref name) = p.name {
                output::kv("  Name", name);
            }
            if let Some(ref tz) = p.timezone {
                output::kv("  Timezone", tz);
            }
            if !p.tech_stack.is_empty() {
                output::kv("  Tech stack", &p.tech_stack.join(", "));
            }
            if !p.projects.is_empty() {
                output::kv(
                    "  Projects",
                    &p.projects
                        .iter()
                        .map(|p| {
                            let mut s = p.name.clone();
                            if let Some(ref lang) = p.language {
                                s.push_str(&format!(" [{lang}]"));
                            }
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if !p.style_preferences.is_empty() {
                output::kv("  Style", &p.style_preferences.join(", "));
            }
            if !p.recurring_tasks.is_empty() {
                output::kv(
                    "  Recurring tasks",
                    &p.recurring_tasks
                        .iter()
                        .map(|t| {
                            let mut s = t.description.clone();
                            if let Some(ref freq) = t.frequency {
                                s.push_str(&format!(" ({freq})"));
                            }
                            s
                        })
                        .collect::<Vec<_>>()
                        .join(", "),
                );
            }
            if !p.schedule_hints.is_empty() {
                output::kv("  Schedule", &p.schedule_hints.join(", "));
            }
            if !p.notes.is_empty() {
                output::kv("  Notes", &p.notes.join("; "));
            }
            println!();
            output::kv("  Revision", &p.revision.to_string());
            output::kv(
                "  Updated",
                &p.updated_at.format("%Y-%m-%d %H:%M:%S").to_string(),
            );
        }
        _ => {
            println!("  No user profile yet. Facts will accumulate and a profile");
            println!("  will be extracted automatically, or run:");
            println!("    aivyx memory profile extract");
        }
    }

    Ok(())
}

pub async fn profile_extract(agent_name: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let config = aivyx_config::AivyxConfig::load(dirs.config_path())?;

    // We need the raw master key for API key resolution and the derived
    // memory key for the memory store.
    let raw_master_key = unlock_raw_master_key(&dirs)?;
    let memory_key = derive_memory_key(&raw_master_key);

    // Create an LLM provider for profile extraction.
    let enc_store = aivyx_crypto::EncryptedStore::open(dirs.store_path())?;
    let agent_profile =
        aivyx_agent::AgentProfile::load(dirs.agents_dir().join(format!("{agent_name}.toml"))).ok();
    let provider_config =
        config.resolve_provider(agent_profile.and_then(|p| p.provider).as_deref());
    let provider = aivyx_llm::create_provider(provider_config, &enc_store, &raw_master_key)?;

    // Build the memory manager
    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let embedding_config = config
        .embedding
        .as_ref()
        .ok_or_else(|| AivyxError::NotInitialized("no embedding provider configured".into()))?;
    let embedding_provider: std::sync::Arc<dyn aivyx_llm::EmbeddingProvider> = std::sync::Arc::from(
        aivyx_llm::create_embedding_provider(embedding_config, &enc_store, &raw_master_key)?,
    );
    let mgr = aivyx_memory::MemoryManager::new(
        store,
        embedding_provider,
        memory_key,
        config.memory.max_memories,
    )?;

    output::header("Extracting user profile...");
    let profile = mgr.extract_profile(provider.as_ref()).await?;

    if profile.is_empty() {
        println!("  No facts or preferences found to extract from.");
    } else {
        output::success("Profile extracted successfully");
        println!();
        if let Some(ref name) = profile.name {
            output::kv("  Name", name);
        }
        if let Some(ref tz) = profile.timezone {
            output::kv("  Timezone", tz);
        }
        if !profile.tech_stack.is_empty() {
            output::kv("  Tech stack", &profile.tech_stack.join(", "));
        }
        output::kv("  Revision", &profile.revision.to_string());
    }

    Ok(())
}

pub fn profile_set(field: &str, value: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_memory_key(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;
    let mut profile = store.load_profile(&master_key)?.unwrap_or_default();

    match field {
        "name" => profile.name = Some(value.to_string()),
        "timezone" => profile.timezone = Some(value.to_string()),
        _ => {
            return Err(AivyxError::Config(format!(
                "unsupported profile field: {field} (supported: name, timezone)"
            )));
        }
    }

    profile.updated_at = chrono::Utc::now();
    profile.revision = profile.revision.saturating_add(1);
    store.save_profile(&profile, &master_key)?;

    output::success(&format!("Set profile {field} = {value}"));
    Ok(())
}

pub fn profile_clear() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    check_initialized(&dirs)?;
    let master_key = unlock_memory_key(&dirs)?;

    let store = MemoryStore::open(dirs.memory_dir().join("memory.db"))?;

    // Save snapshot before clearing
    if let Some(current) = store.load_profile(&master_key)? {
        store.save_profile_snapshot(&current, current.revision, &master_key)?;
    }

    let empty = aivyx_memory::UserProfile::new();
    store.save_profile(&empty, &master_key)?;
    store.save_extraction_counter(0, &master_key)?;

    output::success("User profile cleared");
    Ok(())
}

fn check_initialized(dirs: &AivyxDirs) -> Result<()> {
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Err(AivyxError::NotInitialized(
            "run `aivyx genesis` first".into(),
        ));
    }
    Ok(())
}

fn unlock_memory_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    let master = crate::unlock::unlock_raw_master_key(dirs)?;
    Ok(derive_memory_key(&master))
}

fn unlock_raw_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_raw_master_key(dirs)
}

fn truncate(s: &str, max_len: usize) -> String {
    if s.len() <= max_len {
        s.to_string()
    } else {
        let end = s.floor_char_boundary(max_len);
        format!("{}...", &s[..end])
    }
}
