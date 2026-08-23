//! Writing provider API keys to the secrets file.
//!
//! One implementation, shared. This lived in the API layer, which the kernel
//! cannot call, so giving agents a "set a provider key" tool would have meant a
//! second copy -- and two copies of a rule drift apart. That is not
//! hypothetical here: the model-id prefix rule existed in two places, they
//! disagreed, and the disagreement corrupted every NVIDIA model id.
//!
//! The file is hardened to the current user after every write. If that fails
//! the write fails, rather than leaving plaintext keys readable to other
//! accounts on the machine.

pub fn write_secret_env(path: &std::path::Path, key: &str, value: &str) -> Result<(), std::io::Error> {
    let mut lines: Vec<String> = if path.exists() {
        std::fs::read_to_string(path)?
            .lines()
            .map(|l| l.to_string())
            .collect()
    } else {
        Vec::new()
    };

    // Remove existing line for this key
    lines.retain(|l| !l.starts_with(&format!("{key}=")));

    // Add new line
    lines.push(format!("{key}={value}"));

    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    std::fs::write(path, lines.join("\n") + "\n")?;

    // SECURITY (threat-model M2/M3): restrict the secrets file to the current user.
    // Unix: set 0600 permissions. Windows: remove inherited ACEs and ensure the
    // current user has Full Control. If hardening fails, return an error rather
    // than leaving plaintext keys readable to other accounts.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(windows)]
    {
        let user = match (std::env::var("USERDOMAIN"), std::env::var("USERNAME")) {
            (Ok(domain), Ok(name)) if !name.is_empty() => format!(r"{domain}\\{name}"),
            (_, Ok(name)) if !name.is_empty() => name,
            _ => String::new(),
        };
        if !user.is_empty() {
            let system_root =
                std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
            let icacls = std::path::PathBuf::from(system_root)
                .join("System32")
                .join("icacls.exe");
            let out = std::process::Command::new(icacls)
                .arg(path)
                .args(["/inheritance:r", "/grant:r", &format!("{user}:F")])
                .output()
                .map_err(|e| std::io::Error::other(format!("icacls failed: {e}")))?;
            if !out.status.success() {
                return Err(std::io::Error::other(format!(
                    "icacls failed: {}",
                    String::from_utf8_lossy(&out.stderr).trim()
                )));
            }
        }
    }

    Ok(())
}
