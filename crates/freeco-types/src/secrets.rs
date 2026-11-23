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

/// Reject a secrets.env entry that could break out of its own line.
///
/// secrets.env is loaded into the process environment at startup, so a value
/// containing a newline would let the caller define arbitrary extra variables
/// (e.g. `LD_PRELOAD`) that every child process then inherits. Keys must be a
/// plain env var name; `std::env::set_var` also panics on `=` or NUL in a key.
pub fn validate_secret_env_entry(key: &str, value: &str) -> Result<(), String> {
    if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(format!(
            "Invalid variable name '{key}': only letters, digits and '_' are allowed"
        ));
    }
    if value.contains(['\n', '\r', '\0']) {
        return Err("Value must not contain line breaks or NUL characters".to_string());
    }
    Ok(())
}

/// Write or update a key in the secrets.env file.
/// File format: one `KEY=value` per line. Existing keys are overwritten.
/// Fails with `ErrorKind::InvalidInput` if the entry is rejected by
/// [`validate_secret_env_entry`].
pub fn write_secret_env(
    path: &std::path::Path,
    key: &str,
    value: &str,
) -> Result<(), std::io::Error> {
    validate_secret_env_entry(key, value)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidInput, e))?;

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_rejects_injection_and_bad_names() {
        assert!(validate_secret_env_entry("GROQ_API_KEY", "sk-x\nLD_PRELOAD=/x.so").is_err());
        assert!(validate_secret_env_entry("GROQ_API_KEY", "sk-x\r").is_err());
        for key in ["", "A=B", "A\nB", "A B", "A\0B"] {
            assert!(validate_secret_env_entry(key, "v").is_err(), "{key:?}");
        }
        assert!(validate_secret_env_entry("MY_PROVIDER_API_KEY", "sk-abc=123").is_ok());
    }
}
