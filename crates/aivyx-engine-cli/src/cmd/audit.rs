use aivyx_audit::{AuditEvent, AuditFilter, AuditLog};
use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, Result};
use aivyx_crypto::{MasterKey, derive_audit_key};
use chrono::DateTime;

use crate::output;

pub fn show(last: usize) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);

    let entries = log.recent(last)?;

    if entries.is_empty() {
        println!("  No audit entries.");
        return Ok(());
    }

    output::header(&format!("Last {} audit entries", entries.len()));
    for entry in &entries {
        let event_json = serde_json::to_string(&entry.event).unwrap_or_else(|_| "???".into());
        println!(
            "  #{:<4} {}  {}",
            entry.sequence_number, entry.timestamp, event_json
        );
    }
    println!();
    Ok(())
}

pub fn verify() -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);

    let result = log.verify()?;

    output::header("Audit verification");
    output::kv("Entries checked", &result.entries_checked.to_string());
    if result.valid {
        output::success("Chain integrity verified — all entries valid.");
    } else {
        output::error("INTEGRITY VIOLATION DETECTED — audit chain is broken!");
    }

    // Record that verification was performed.
    log.append(AuditEvent::AuditVerified {
        entries_checked: result.entries_checked,
        valid: result.valid,
    })?;

    println!();
    Ok(())
}

/// Export audit log entries to a file or stdout.
pub fn export(format: &str, output_path: Option<&std::path::Path>) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);

    let mut writer: Box<dyn std::io::Write> = match output_path {
        Some(path) => Box::new(std::fs::File::create(path).map_err(AivyxError::Io)?),
        None => Box::new(std::io::stdout()),
    };

    match format {
        "json" => aivyx_audit::export_json(&log, &mut writer)?,
        "csv" => aivyx_audit::export_csv(&log, &mut writer)?,
        other => {
            output::error(&format!("unknown format: {other} (expected json or csv)"));
            return Ok(());
        }
    }

    if let Some(path) = output_path {
        output::success(&format!("Exported audit log to {}", path.display()));
    }
    Ok(())
}

/// Search audit log entries by type and date range.
pub fn search(
    event_type: Option<&str>,
    from: Option<&str>,
    to: Option<&str>,
    limit: usize,
) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);

    let filter = AuditFilter {
        event_types: event_type.map(|t| vec![t.to_string()]),
        from: from.and_then(|s| DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.to_utc())),
        to: to.and_then(|s| DateTime::parse_from_rfc3339(s).ok().map(|dt| dt.to_utc())),
        limit: Some(limit),
    };

    let results = log.search(&filter)?;

    if results.is_empty() {
        println!("  No matching entries.");
        return Ok(());
    }

    output::header(&format!("Found {} matching entries", results.len()));
    for entry in &results {
        let event_json = serde_json::to_string(&entry.event).unwrap_or_else(|_| "???".into());
        println!(
            "  #{:<4} {}  {}",
            entry.sequence_number, entry.timestamp, event_json
        );
    }
    println!();
    Ok(())
}

/// Prune audit entries older than a given date.
pub fn prune(before: &str) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;
    if !dirs.is_initialized() {
        output::error("aivyx is not initialized. Run `aivyx genesis` to get started.");
        return Ok(());
    }

    let cutoff = DateTime::parse_from_rfc3339(before)
        .map_err(|e| AivyxError::Config(format!("invalid date format: {e}")))?
        .to_utc();

    let master_key = unlock_master_key(&dirs)?;
    let audit_key = derive_audit_key(&master_key);
    let log = AuditLog::new(dirs.audit_path(), &audit_key);

    // Verify integrity before pruning
    let verify_result = log.verify()?;
    if !verify_result.valid {
        output::error("Audit log integrity check failed. Cannot prune a corrupted log.");
        return Ok(());
    }

    let result = aivyx_audit::prune(&log, cutoff)?;

    output::header("Audit log pruned");
    output::kv("Entries removed", &result.entries_removed.to_string());
    output::kv("Entries remaining", &result.entries_remaining.to_string());
    output::success("Log pruned successfully. HMAC chain re-established.");
    println!();
    Ok(())
}

fn unlock_master_key(dirs: &AivyxDirs) -> Result<MasterKey> {
    crate::unlock::unlock_master_key(dirs)
}
