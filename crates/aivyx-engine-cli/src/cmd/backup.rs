use std::path::Path;

use aivyx_config::AivyxDirs;
use aivyx_core::{AivyxError, Result};
use flate2::Compression;
use flate2::read::GzDecoder;
use flate2::write::GzEncoder;
use tar::Archive;

use crate::output;

/// Create a tar.gz backup of the `~/.aivyx/` data directory.
pub fn create(output_path: &Path) -> Result<()> {
    let dirs = AivyxDirs::from_default()?;

    if !dirs.is_initialized() {
        return Err(AivyxError::Config(
            "aivyx is not initialized -- nothing to back up".into(),
        ));
    }

    output::header("Creating backup");

    let file = std::fs::File::create(output_path)?;
    let encoder = GzEncoder::new(file, Compression::default());
    let mut builder = tar::Builder::new(encoder);

    builder
        .append_dir_all("aivyx", dirs.root())
        .map_err(|e| AivyxError::Other(format!("failed to archive aivyx directory: {e}")))?;

    let encoder = builder
        .into_inner()
        .map_err(|e| AivyxError::Other(format!("failed to finalize archive: {e}")))?;
    encoder
        .finish()
        .map_err(|e| AivyxError::Other(format!("failed to finish gzip compression: {e}")))?;

    output::success(&format!("Backup written to {}", output_path.display()));
    Ok(())
}

/// Restore the `~/.aivyx/` data directory from a tar.gz backup archive.
pub fn restore(archive_path: &Path) -> Result<()> {
    if !archive_path.exists() {
        return Err(AivyxError::Config(format!(
            "archive not found: {}",
            archive_path.display()
        )));
    }

    output::header("Restoring from backup");

    let dirs = AivyxDirs::from_default()?;
    let target = dirs.root().to_path_buf();

    // Extract to a temporary directory first for verification.
    let temp_dir = tempfile::tempdir()
        .map_err(|e| AivyxError::Other(format!("failed to create temp directory: {e}")))?;

    let file = std::fs::File::open(archive_path)?;
    let decoder = GzDecoder::new(file);
    let mut archive = Archive::new(decoder);

    archive
        .unpack(temp_dir.path())
        .map_err(|e| AivyxError::Other(format!("failed to extract archive: {e}")))?;

    // The archive contains an "aivyx/" prefix, so the extracted content
    // lives at temp_dir/aivyx/.
    let extracted = temp_dir.path().join("aivyx");

    // Verify expected structure exists.
    if !extracted.join("config.toml").exists() {
        return Err(AivyxError::Config(
            "invalid backup: config.toml not found in archive".into(),
        ));
    }
    if !extracted.join("keys").exists() {
        return Err(AivyxError::Config(
            "invalid backup: keys/ directory not found in archive".into(),
        ));
    }

    // Remove old data directory and move extracted content into place.
    if target.exists() {
        std::fs::remove_dir_all(&target)?;
    }

    // Rename from temp location to the real data directory.
    // If rename fails (e.g. cross-device), fall back to a recursive copy.
    if std::fs::rename(&extracted, &target).is_err() {
        copy_dir_recursive(&extracted, &target)?;
    }

    output::success(&format!("Restored aivyx data to {}", target.display()));
    Ok(())
}

/// Recursively copy a directory tree from `src` to `dst`.
fn copy_dir_recursive(src: &Path, dst: &Path) -> Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let file_type = entry.file_type()?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if file_type.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn backup_creates_valid_archive() {
        let src_dir = tempfile::tempdir().unwrap();
        let src_path = src_dir.path();

        // Create a test file inside the source directory.
        fs::write(src_path.join("test.txt"), "hello backup").unwrap();

        let archive_dir = tempfile::tempdir().unwrap();
        let archive_path = archive_dir.path().join("test-backup.tar.gz");

        // Manually create an archive of src_dir (simulating what `create` does).
        let file = fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder.append_dir_all("aivyx", src_path).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();

        // Verify the archive is non-empty.
        let metadata = fs::metadata(&archive_path).unwrap();
        assert!(metadata.len() > 0);

        // Verify the archive can be read back and contains the expected file.
        let file = fs::File::open(&archive_path).unwrap();
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        let entries: Vec<String> = archive
            .entries()
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.path().unwrap().display().to_string())
            .collect();

        assert!(entries.iter().any(|p| p.contains("test.txt")));
    }

    #[test]
    fn backup_restore_round_trip() {
        let fake_aivyx = tempfile::tempdir().unwrap();
        let fake_root = fake_aivyx.path();

        // Create a minimal structure that mimics ~/.aivyx/.
        fs::create_dir_all(fake_root.join("keys")).unwrap();
        fs::write(
            fake_root.join("config.toml"),
            "[provider]\nbackend = \"claude\"",
        )
        .unwrap();
        fs::write(fake_root.join("keys/master.json"), "{}").unwrap();

        // Create the backup archive.
        let archive_dir = tempfile::tempdir().unwrap();
        let archive_path = archive_dir.path().join("roundtrip.tar.gz");

        let file = fs::File::create(&archive_path).unwrap();
        let encoder = GzEncoder::new(file, Compression::default());
        let mut builder = tar::Builder::new(encoder);
        builder.append_dir_all("aivyx", fake_root).unwrap();
        let encoder = builder.into_inner().unwrap();
        encoder.finish().unwrap();

        // Extract to a new temp directory to simulate restore.
        let restore_temp = tempfile::tempdir().unwrap();
        let file = fs::File::open(&archive_path).unwrap();
        let decoder = GzDecoder::new(file);
        let mut archive = Archive::new(decoder);
        archive.unpack(restore_temp.path()).unwrap();

        let restored = restore_temp.path().join("aivyx");
        assert!(restored.join("config.toml").exists());
        assert!(restored.join("keys").exists());
        assert!(restored.join("keys/master.json").exists());

        let content = fs::read_to_string(restored.join("config.toml")).unwrap();
        assert!(content.contains("claude"));
    }
}
