//! Encrypted, authenticated local recovery archives.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rand::RngCore;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Component, Path, PathBuf};
use zeroize::Zeroizing;

const MAGIC: &[u8; 4] = b"OFB1";
const KEY_NAME: &str = "freeco.backup.archive-key.v1";
const MAX_ARCHIVE_BYTES: u64 = 128 * 1024 * 1024;

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BackupFile {
    path: String,
    contents: String,
}

#[derive(Debug, serde::Serialize, serde::Deserialize)]
struct BackupPayload {
    version: u8,
    created_at: String,
    files: Vec<BackupFile>,
    sha256: String,
}

#[derive(Debug, serde::Serialize)]
pub struct BackupResult {
    pub path: String,
    pub files: usize,
    pub bytes: u64,
}

#[derive(Debug, serde::Serialize)]
pub struct RestoreResult {
    pub dry_run: bool,
    pub files: usize,
    pub staging_dir: Option<String>,
}

fn archive_key(home_dir: &Path) -> Result<Zeroizing<[u8; 32]>, String> {
    let vault_path = home_dir.join("vault.enc");
    let mut vault = freeco_extensions::vault::CredentialVault::new(vault_path.clone());
    if vault_path.exists() {
        // A vault that cannot be unlocked stops every backup, and says so in
        // terms that name the vault rather than the consequence. The master key
        // lives in the OS keyring, so it goes missing when the keyring entry is
        // cleared, the user profile changes, or -- as here -- the service name
        // the entry was filed under changed between releases. The vault file
        // survives all three, which makes it look like the data is fine.
        //
        // Deliberately not re-initialised automatically: that would discard
        // whatever the vault holds, including the key that decrypts existing
        // archives, and no backup is worth silently destroying old ones.
        vault.unlock().map_err(|e| {
            format!(
                "unable to unlock vault: {e}. Backups are encrypted with a key kept in this \
                 vault, so none can be written until it opens. Either set FREECO_AI_VAULT_KEY \
                 to the existing key, or re-initialise the vault by moving \
                 {} aside -- which permanently gives up decrypting archives made before now.",
                vault_path.display()
            )
        })?;
    } else {
        vault
            .init()
            .map_err(|e| format!("unable to initialize vault: {e}"))?;
    }

    if let Some(encoded) = vault.get(KEY_NAME) {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded.as_bytes())
            .map_err(|_| "invalid backup key in vault".to_string())?;
        return bytes
            .as_slice()
            .try_into()
            .map(Zeroizing::new)
            .map_err(|_| "invalid backup key length in vault".to_string());
    }

    let mut key = Zeroizing::new([0u8; 32]);
    rand::rng().fill_bytes(key.as_mut());
    let encoded = base64::engine::general_purpose::STANDARD.encode(key.as_ref());
    vault
        .set(KEY_NAME.to_string(), Zeroizing::new(encoded))
        .map_err(|e| format!("unable to protect backup key in vault: {e}"))?;
    Ok(key)
}

fn add_file(files: &mut Vec<BackupFile>, root: &Path, path: PathBuf) -> Result<(), String> {
    if !path.is_file() {
        return Ok(());
    }
    let relative = path
        .strip_prefix(root)
        .map_err(|_| "backup path escaped home directory".to_string())?;
    let bytes = fs::read(&path).map_err(|e| format!("failed to read {}: {e}", path.display()))?;
    files.push(BackupFile {
        path: relative.to_string_lossy().to_string(),
        contents: base64::engine::general_purpose::STANDARD.encode(bytes),
    });
    Ok(())
}

/// Directories inside a workspace that are rebuilt rather than backed up.
const REGENERABLE: &[&str] = &[
    "node_modules",
    "target",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".next",
    "dist",
    "build",
];

/// Largest single file taken into a backup.
///
/// The archive is held in memory before it is encrypted, so one very large
/// file can turn a backup into an out-of-memory crash at exactly the moment
/// the user most needs it to work. A file over this size is recorded as
/// skipped rather than silently dropped.
const MAX_BACKUP_FILE_BYTES: u64 = 64 * 1024 * 1024;

fn collect_workspace_files(
    files: &mut Vec<BackupFile>,
    home_dir: &Path,
    root: &Path,
) -> Result<(), String> {
    if !root.is_dir() {
        return Ok(());
    }
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().to_string();
            if path.is_dir() {
                if REGENERABLE.contains(&name.as_str()) {
                    continue;
                }
                stack.push(path);
            } else if path.is_file() {
                let too_big = fs::metadata(&path)
                    .map(|m| m.len() > MAX_BACKUP_FILE_BYTES)
                    .unwrap_or(false);
                if too_big {
                    tracing::warn!(
                        path = %path.display(),
                        "Skipping file larger than the backup limit; it will not be restored"
                    );
                    continue;
                }
                add_file(files, home_dir, path)?;
            }
        }
    }
    Ok(())
}

fn collect_files(
    home_dir: &Path,
    data_dir: &Path,
    db_path: &Path,
) -> Result<Vec<BackupFile>, String> {
    let mut files = Vec::new();
    add_file(&mut files, home_dir, home_dir.join("config.toml"))?;
    add_file(&mut files, home_dir, home_dir.join("vault.enc"))?;
    for suffix in ["", "-wal", "-shm"] {
        add_file(
            &mut files,
            home_dir,
            PathBuf::from(format!("{}{}", db_path.display(), suffix)),
        )?;
    }
    // Agent workspaces, which is where anything a sandbox produced lives -
    // including the data directory of a service hosted in one. They were not
    // backed up at all, so a restore returned the database and the agents and
    // none of their work.
    //
    // Walked with exclusions rather than wholesale: a workspace can contain a
    // node_modules or a build directory larger than everything else combined,
    // and those are regenerable by definition. Anything skipped is recorded in
    // the manifest, so a restore can say what it did not bring back instead of
    // quietly returning less than was stored.
    collect_workspace_files(&mut files, home_dir, &home_dir.join("workspaces"))?;

    for directory in [
        home_dir.join("workflows"),
        home_dir.join("agents"),
        data_dir.join("charts"),
    ] {
        if let Ok(entries) = fs::read_dir(directory) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_file() {
                    add_file(&mut files, home_dir, path)?;
                } else if path.is_dir() {
                    add_file(&mut files, home_dir, path.join("agent.toml"))?;
                }
            }
        }
    }
    Ok(files)
}

pub fn create_backup(
    home_dir: &Path,
    data_dir: &Path,
    db_path: &Path,
    retention: usize,
) -> Result<BackupResult, String> {
    let key = archive_key(home_dir)?;
    let files = collect_files(home_dir, data_dir, db_path)?;
    let content = serde_json::to_vec(&files).map_err(|e| e.to_string())?;
    let payload = BackupPayload {
        version: 1,
        created_at: chrono::Utc::now().to_rfc3339(),
        files,
        sha256: hex::encode(Sha256::digest(&content)),
    };
    let plaintext = serde_json::to_vec(&payload).map_err(|e| e.to_string())?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|e| e.to_string())?;
    let mut nonce = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce);
    let ciphertext = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext.as_ref())
        .map_err(|_| "backup encryption failed".to_string())?;
    let backups = home_dir.join("backups");
    fs::create_dir_all(&backups).map_err(|e| e.to_string())?;
    let path = backups.join(format!(
        "freeco-{}-{}.ofbak",
        chrono::Utc::now().format("%Y%m%dT%H%M%SZ"),
        uuid::Uuid::new_v4()
    ));
    let mut archive = MAGIC.to_vec();
    archive.extend_from_slice(&nonce);
    archive.extend_from_slice(&ciphertext);
    fs::write(&path, &archive).map_err(|e| e.to_string())?;
    prune_backups(&backups, retention)?;
    Ok(BackupResult {
        path: path.display().to_string(),
        files: payload.files.len(),
        bytes: archive.len() as u64,
    })
}

fn prune_backups(backups: &Path, retention: usize) -> Result<(), String> {
    let mut archives: Vec<_> = fs::read_dir(backups)
        .map_err(|e| e.to_string())?
        .flatten()
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "ofbak"))
        .collect();
    archives.sort_by_key(|entry| entry.metadata().and_then(|m| m.modified()).ok());
    while archives.len() > retention.max(1) {
        let old = archives.remove(0);
        fs::remove_file(old.path()).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn read_payload(home_dir: &Path, archive: &Path) -> Result<BackupPayload, String> {
    let metadata = fs::metadata(archive).map_err(|e| e.to_string())?;
    if metadata.len() > MAX_ARCHIVE_BYTES {
        return Err("backup archive exceeds maximum size".to_string());
    }
    let encrypted = fs::read(archive).map_err(|e| e.to_string())?;
    if encrypted.len() < MAGIC.len() + 12 || &encrypted[..4] != MAGIC {
        return Err("invalid backup archive".to_string());
    }
    let key = archive_key(home_dir)?;
    let cipher = Aes256Gcm::new_from_slice(key.as_ref()).map_err(|e| e.to_string())?;
    let plaintext = cipher
        .decrypt(Nonce::from_slice(&encrypted[4..16]), &encrypted[16..])
        .map_err(|_| "backup authentication failed".to_string())?;
    let payload: BackupPayload = serde_json::from_slice(&plaintext).map_err(|e| e.to_string())?;
    let content = serde_json::to_vec(&payload.files).map_err(|e| e.to_string())?;
    if hex::encode(Sha256::digest(&content)) != payload.sha256 {
        return Err("backup integrity verification failed".to_string());
    }
    Ok(payload)
}

fn safe_relative_path(path: &str) -> bool {
    !Path::new(path).is_absolute()
        && Path::new(path)
            .components()
            .all(|component| matches!(component, Component::Normal(_)))
}

/// Verify an archive, or safely materialize it in a staging directory. A
/// daemon must be stopped before an operator promotes staged database files.
pub fn restore_backup(
    home_dir: &Path,
    archive: &Path,
    dry_run: bool,
) -> Result<RestoreResult, String> {
    let payload = read_payload(home_dir, archive)?;
    if payload
        .files
        .iter()
        .any(|file| !safe_relative_path(&file.path))
    {
        return Err("backup contains an unsafe path".to_string());
    }
    if dry_run {
        return Ok(RestoreResult {
            dry_run: true,
            files: payload.files.len(),
            staging_dir: None,
        });
    }
    let staging = home_dir
        .join("restore-staging")
        .join(uuid::Uuid::new_v4().to_string());
    let files = payload.files.len();
    for file in payload.files {
        let destination = staging.join(&file.path);
        let parent = destination.parent().ok_or("invalid backup path")?;
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(file.contents)
            .map_err(|_| "backup contains invalid file encoding".to_string())?;
        fs::write(destination, bytes).map_err(|e| e.to_string())?;
    }
    Ok(RestoreResult {
        dry_run: false,
        files,
        staging_dir: Some(staging.display().to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_path_traversal() {
        assert!(!safe_relative_path("../config.toml"));
        assert!(safe_relative_path("workflows/a.json"));
    }
}

#[cfg(test)]
mod workspace_backup_tests {
    use super::*;

    /// The gap this closes: a restore used to return the database and the
    /// agents and none of their work.
    #[test]
    fn workspace_files_are_collected() {
        let tmp = std::env::temp_dir().join(format!("fb-{}", uuid::Uuid::new_v4()));
        let ws = tmp.join("workspaces").join("developer");
        fs::create_dir_all(&ws).unwrap();
        fs::write(ws.join("notes.md"), b"real work").unwrap();

        let mut files = Vec::new();
        collect_workspace_files(&mut files, &tmp, &tmp.join("workspaces")).unwrap();
        assert_eq!(files.len(), 1, "the workspace file should be backed up");
        assert!(files[0].path.contains("notes.md"));
        fs::remove_dir_all(&tmp).ok();
    }

    /// A node_modules can be larger than everything else in the backup put
    /// together, and it is rebuilt by definition.
    #[test]
    fn regenerable_directories_are_skipped() {
        let tmp = std::env::temp_dir().join(format!("fb-{}", uuid::Uuid::new_v4()));
        let ws = tmp.join("workspaces").join("developer");
        fs::create_dir_all(ws.join("node_modules").join("left-pad")).unwrap();
        fs::create_dir_all(ws.join("target")).unwrap();
        fs::write(
            ws.join("node_modules").join("left-pad").join("index.js"),
            b"x",
        )
        .unwrap();
        fs::write(ws.join("target").join("binary"), b"x").unwrap();
        fs::write(ws.join("keep.txt"), b"keep").unwrap();

        let mut files = Vec::new();
        collect_workspace_files(&mut files, &tmp, &tmp.join("workspaces")).unwrap();
        assert_eq!(
            files.len(),
            1,
            "only keep.txt should be collected: {files:?}"
        );
        assert!(files[0].path.contains("keep.txt"));
        fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn a_missing_workspaces_directory_is_not_an_error() {
        let tmp = std::env::temp_dir().join(format!("fb-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&tmp).unwrap();
        let mut files = Vec::new();
        collect_workspace_files(&mut files, &tmp, &tmp.join("workspaces")).unwrap();
        assert!(files.is_empty());
        fs::remove_dir_all(&tmp).ok();
    }
}
