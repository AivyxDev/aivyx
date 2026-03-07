//! Backup and restore utilities for the aivyx data directory.
//!
//! Creates timestamped tar.gz archives and prunes backups older than a
//! configurable retention period.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::{Deserialize, Serialize};
use tracing::{debug, info, warn};

/// Backup configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BackupConfig {
    /// Whether automatic backups are enabled.
    pub enabled: bool,
    /// Cron expression for the backup schedule.
    pub schedule: String,
    /// Destination directory for backup archives (local path).
    pub destination: String,
    /// Number of days to retain backups before pruning.
    pub retention_days: u32,
}

impl Default for BackupConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            schedule: "0 2 * * *".into(), // daily at 2am
            destination: String::new(),
            retention_days: 30,
        }
    }
}

/// Filename prefix used for backup archives.
const BACKUP_PREFIX: &str = "aivyx-backup-";

/// Filename suffix for backup archives.
const BACKUP_SUFFIX: &str = ".tar.gz";

/// Create a backup of the data directory.
///
/// Creates a tar.gz archive of the specified `data_dir` contents inside
/// `dest_dir`. Temporary files (`*.tmp`), lock files (`*.lock`), and
/// directories named `tmp` are excluded.
///
/// Returns the path to the created backup archive.
pub fn create_backup(data_dir: &Path, dest_dir: &Path) -> aivyx_core::Result<PathBuf> {
    if !data_dir.exists() {
        return Err(aivyx_core::AivyxError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("data directory does not exist: {}", data_dir.display()),
        )));
    }

    fs::create_dir_all(dest_dir)?;

    let timestamp = Utc::now().format("%Y%m%d-%H%M%S");
    let filename = format!("{BACKUP_PREFIX}{timestamp}{BACKUP_SUFFIX}");
    let archive_path = dest_dir.join(&filename);

    info!(
        data_dir = %data_dir.display(),
        archive = %archive_path.display(),
        "creating backup"
    );

    let file = fs::File::create(&archive_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut archive = tar::Builder::new(encoder);

    // Walk the data directory and add files, excluding temporaries and locks.
    append_dir_filtered(&mut archive, data_dir, data_dir)?;

    let encoder = archive.into_inner()?;
    encoder.finish()?;

    let size = fs::metadata(&archive_path)?.len();
    info!(
        archive = %archive_path.display(),
        size_bytes = size,
        "backup created"
    );

    Ok(archive_path)
}

/// Recursively append directory contents to a tar archive, filtering out
/// temporary and lock files.
fn append_dir_filtered<W: std::io::Write>(
    archive: &mut tar::Builder<W>,
    dir: &Path,
    base: &Path,
) -> aivyx_core::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        // Skip temp files, lock files, and tmp directories.
        if name_str.ends_with(".tmp")
            || name_str.ends_with(".lock")
            || name_str == "tmp"
            || name_str.starts_with('.')
        {
            debug!(path = %path.display(), "skipping excluded path");
            continue;
        }

        let relative = path
            .strip_prefix(base)
            .map_err(|e| aivyx_core::AivyxError::Io(std::io::Error::other(e)))?;

        if path.is_dir() {
            append_dir_filtered(archive, &path, base)?;
        } else if path.is_file() {
            archive.append_path_with_name(&path, relative)?;
        }
    }
    Ok(())
}

/// Remove backups older than `retention_days` from the destination directory.
///
/// Only files matching the `aivyx-backup-*.tar.gz` naming pattern are
/// considered. Returns the count of removed backup files.
pub fn prune_backups(dest_dir: &Path, retention_days: u32) -> aivyx_core::Result<u32> {
    if !dest_dir.exists() {
        return Ok(0);
    }

    let cutoff = Utc::now() - chrono::Duration::days(i64::from(retention_days));
    let mut removed = 0u32;

    for entry in fs::read_dir(dest_dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let name_str = name.to_string_lossy();

        if !name_str.starts_with(BACKUP_PREFIX) || !name_str.ends_with(BACKUP_SUFFIX) {
            continue;
        }

        // Parse timestamp from filename: aivyx-backup-YYYYMMDD-HHMMSS.tar.gz
        let ts_part = &name_str[BACKUP_PREFIX.len()..name_str.len() - BACKUP_SUFFIX.len()];
        let parsed = chrono::NaiveDateTime::parse_from_str(ts_part, "%Y%m%d-%H%M%S");

        match parsed {
            Ok(naive) => {
                let file_time = naive.and_utc();
                if file_time < cutoff {
                    let path = entry.path();
                    info!(path = %path.display(), age_days = (Utc::now() - file_time).num_days(), "pruning old backup");
                    fs::remove_file(&path)?;
                    removed += 1;
                }
            }
            Err(e) => {
                warn!(
                    file = %name_str,
                    error = %e,
                    "skipping file with unparseable timestamp"
                );
            }
        }
    }

    if removed > 0 {
        info!(removed, "backup pruning complete");
    }

    Ok(removed)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Helper: create a temporary directory with some test files.
    fn setup_test_data(base: &Path) {
        let sub = base.join("agents");
        fs::create_dir_all(&sub).unwrap();
        fs::write(base.join("config.toml"), b"[provider]\ntype = \"ollama\"").unwrap();
        fs::write(sub.join("assistant.yaml"), b"name: assistant").unwrap();
        // These should be excluded:
        fs::write(base.join("session.lock"), b"locked").unwrap();
        fs::write(base.join("scratch.tmp"), b"temp data").unwrap();
        fs::create_dir_all(base.join("tmp")).unwrap();
        fs::write(base.join("tmp/junk"), b"should be skipped").unwrap();
        fs::write(base.join(".hidden"), b"hidden file").unwrap();
    }

    #[test]
    fn create_backup_produces_valid_tar_gz() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let dest_dir = dir.path().join("backups");
        fs::create_dir_all(&data_dir).unwrap();
        setup_test_data(&data_dir);

        let archive_path = create_backup(&data_dir, &dest_dir).unwrap();

        // Verify archive exists and has correct naming
        assert!(archive_path.exists());
        let name = archive_path.file_name().unwrap().to_string_lossy();
        assert!(name.starts_with(BACKUP_PREFIX));
        assert!(name.ends_with(BACKUP_SUFFIX));

        // Verify it's a valid tar.gz with expected contents
        let file = fs::File::open(&archive_path).unwrap();
        let decoder = flate2::read::GzDecoder::new(file);
        let mut archive = tar::Archive::new(decoder);

        let mut found_files: Vec<String> = Vec::new();
        for entry in archive.entries().unwrap() {
            let entry = entry.unwrap();
            let path = entry.path().unwrap().to_string_lossy().to_string();
            found_files.push(path);
        }

        found_files.sort();
        assert!(
            found_files.contains(&"config.toml".to_string()),
            "should contain config.toml, found: {found_files:?}"
        );
        assert!(
            found_files.contains(&"agents/assistant.yaml".to_string()),
            "should contain agents/assistant.yaml, found: {found_files:?}"
        );

        // Excluded files should NOT be present
        for f in &found_files {
            assert!(!f.ends_with(".lock"), "lock files should be excluded: {f}");
            assert!(!f.ends_with(".tmp"), "tmp files should be excluded: {f}");
            assert!(!f.starts_with("tmp/"), "tmp dir should be excluded: {f}");
            assert!(!f.starts_with('.'), "hidden files should be excluded: {f}");
        }
    }

    #[test]
    fn create_backup_missing_data_dir_errors() {
        let dir = tempfile::tempdir().unwrap();
        let result = create_backup(&dir.path().join("nonexistent"), &dir.path().join("backups"));
        assert!(result.is_err());
    }

    #[test]
    fn prune_backups_removes_old_keeps_recent() {
        let dir = tempfile::tempdir().unwrap();
        let dest = dir.path();

        // Create a "recent" backup (today's timestamp)
        let recent_name = format!(
            "{BACKUP_PREFIX}{}{BACKUP_SUFFIX}",
            Utc::now().format("%Y%m%d-%H%M%S")
        );
        fs::write(dest.join(&recent_name), b"recent").unwrap();

        // Create an "old" backup (60 days ago)
        let old_time = Utc::now() - chrono::Duration::days(60);
        let old_name = format!(
            "{BACKUP_PREFIX}{}{BACKUP_SUFFIX}",
            old_time.format("%Y%m%d-%H%M%S")
        );
        fs::write(dest.join(&old_name), b"old").unwrap();

        // Create a non-backup file (should be ignored)
        fs::write(dest.join("notes.txt"), b"unrelated").unwrap();

        let removed = prune_backups(dest, 30).unwrap();

        assert_eq!(removed, 1, "should remove exactly the old backup");
        assert!(
            dest.join(&recent_name).exists(),
            "recent backup should still exist"
        );
        assert!(
            !dest.join(&old_name).exists(),
            "old backup should be removed"
        );
        assert!(
            dest.join("notes.txt").exists(),
            "non-backup files should be untouched"
        );
    }

    #[test]
    fn prune_backups_nonexistent_dir_returns_zero() {
        let dir = tempfile::tempdir().unwrap();
        let result = prune_backups(&dir.path().join("no-such-dir"), 30).unwrap();
        assert_eq!(result, 0);
    }

    #[test]
    fn backup_config_serde_roundtrip() {
        let config = BackupConfig {
            enabled: true,
            schedule: "0 3 * * 0".into(), // weekly at 3am Sunday
            destination: "/backups/aivyx".into(),
            retention_days: 14,
        };

        let json = serde_json::to_string(&config).unwrap();
        let deserialized: BackupConfig = serde_json::from_str(&json).unwrap();

        assert!(deserialized.enabled);
        assert_eq!(deserialized.schedule, "0 3 * * 0");
        assert_eq!(deserialized.destination, "/backups/aivyx");
        assert_eq!(deserialized.retention_days, 14);
    }

    #[test]
    fn backup_config_default() {
        let config = BackupConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.schedule, "0 2 * * *");
        assert!(config.destination.is_empty());
        assert_eq!(config.retention_days, 30);
    }

    #[test]
    fn backup_config_toml_roundtrip() {
        let config = BackupConfig {
            enabled: true,
            schedule: "0 2 * * *".into(),
            destination: "/var/backups".into(),
            retention_days: 7,
        };

        let toml_str = toml::to_string(&config).unwrap();
        let parsed: BackupConfig = toml::from_str(&toml_str).unwrap();

        assert!(parsed.enabled);
        assert_eq!(parsed.schedule, "0 2 * * *");
        assert_eq!(parsed.destination, "/var/backups");
        assert_eq!(parsed.retention_days, 7);
    }

    #[test]
    fn create_and_prune_integration() {
        let dir = tempfile::tempdir().unwrap();
        let data_dir = dir.path().join("data");
        let dest_dir = dir.path().join("backups");
        fs::create_dir_all(&data_dir).unwrap();
        fs::write(data_dir.join("test.txt"), b"hello").unwrap();

        // Create a backup
        let path = create_backup(&data_dir, &dest_dir).unwrap();
        assert!(path.exists());

        // Pruning with 30-day retention should keep it (it was just created)
        let removed = prune_backups(&dest_dir, 30).unwrap();
        assert_eq!(removed, 0);
        assert!(path.exists());

        // Pruning with 0-day retention should remove it
        let removed = prune_backups(&dest_dir, 0).unwrap();
        assert_eq!(removed, 1);
        assert!(!path.exists());
    }
}
