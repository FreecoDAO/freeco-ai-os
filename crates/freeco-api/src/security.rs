//! Central admission control for externally supplied agent and workflow content.

use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

const MAX_AUDITS: usize = 1_000;

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FindingSeverity {
    Info,
    Warning,
    Critical,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SecurityFinding {
    pub severity: FindingSeverity,
    pub rule: &'static str,
    pub message: String,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum SecurityDecision {
    Allowed,
    PendingApproval,
    Quarantined,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SecurityAudit {
    pub id: String,
    pub content_hash: String,
    pub subject: String,
    pub created_at: String,
    pub decision: SecurityDecision,
    pub findings: Vec<SecurityFinding>,
    /// What the content asks for, when it is a TOML manifest: its name and
    /// `[capabilities]` (plus the exec policy mode), so the person approving
    /// it can see what they are approving. `None` for other content.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<serde_json::Value>,
}

/// Name, capabilities and exec policy mode of a TOML manifest. `None` when
/// the content is not TOML or has none of these.
pub fn manifest_summary(content: &str) -> Option<serde_json::Value> {
    let table: toml::Table = content.parse().ok()?;
    let name = table.get("name").and_then(|v| v.as_str());
    let capabilities = table.get("capabilities").and_then(|v| v.as_table());
    let exec_mode = match table.get("exec_policy") {
        Some(toml::Value::String(mode)) => Some(mode.clone()),
        Some(toml::Value::Table(t)) => t.get("mode").and_then(|m| m.as_str()).map(String::from),
        _ => None,
    };
    if name.is_none() && capabilities.is_none() && exec_mode.is_none() {
        return None;
    }
    let capabilities = capabilities
        .and_then(|c| serde_json::to_value(c).ok())
        .unwrap_or(serde_json::Value::Null);
    Some(serde_json::json!({
        "name": name,
        "capabilities": capabilities,
        "exec_policy_mode": exec_mode,
    }))
}

/// In-process security service. Audit results are additionally written to the
/// kernel's append-only audit log by route handlers.
pub struct SecurityService {
    audits: Mutex<VecDeque<SecurityAudit>>,
    approvals: Mutex<HashMap<String, String>>,
}

impl Default for SecurityService {
    fn default() -> Self {
        Self {
            audits: Mutex::new(VecDeque::new()),
            approvals: Mutex::new(HashMap::new()),
        }
    }
}

impl SecurityService {
    pub fn scan(&self, subject: &str, content: &str) -> SecurityAudit {
        let content_hash = hex::encode(Sha256::digest(content.as_bytes()));
        let lower = content.to_ascii_lowercase();
        let mut findings = Vec::new();

        for pattern in [
            "ignore previous instructions",
            "ignore all previous",
            "system prompt override",
            "disregard previous",
        ] {
            if lower.contains(pattern) {
                findings.push(SecurityFinding {
                    severity: FindingSeverity::Critical,
                    rule: "prompt-injection",
                    message: format!("Prompt-injection pattern detected: '{pattern}'"),
                });
            }
        }
        for pattern in ["rm -rf", "curl | sh", "wget | sh", "chmod 777", "sudo "] {
            if lower.contains(pattern) {
                findings.push(SecurityFinding {
                    severity: FindingSeverity::Critical,
                    rule: "unsafe-execution",
                    message: format!("Unsafe execution pattern detected: '{pattern}'"),
                });
            }
        }
        for pattern in ["netconnect(*)", "shell_exec", "shellexec", "full_access"] {
            if lower.contains(pattern) {
                findings.push(SecurityFinding {
                    severity: FindingSeverity::Warning,
                    rule: "privileged-capability",
                    message: format!("Privileged capability requested: '{pattern}'"),
                });
            }
        }
        for marker in [
            "api_key =",
            "secret =",
            "private_key",
            "authorization: bearer",
        ] {
            if lower.contains(marker) {
                findings.push(SecurityFinding {
                    severity: FindingSeverity::Warning,
                    rule: "possible-secret",
                    message: format!("Potential credential material detected: '{marker}'"),
                });
            }
        }

        let has_critical = findings
            .iter()
            .any(|finding| matches!(finding.severity, FindingSeverity::Critical));
        let approved = self
            .approvals
            .lock()
            .expect("security approvals lock poisoned")
            .contains_key(&content_hash);
        let decision = if has_critical {
            SecurityDecision::Quarantined
        } else if findings.is_empty() || approved {
            SecurityDecision::Allowed
        } else {
            SecurityDecision::PendingApproval
        };
        let audit = SecurityAudit {
            id: uuid::Uuid::new_v4().to_string(),
            content_hash,
            subject: subject.to_string(),
            created_at: Utc::now().to_rfc3339(),
            decision,
            findings,
            summary: manifest_summary(content),
        };
        let mut audits = self.audits.lock().expect("security audits lock poisoned");
        audits.push_back(audit.clone());
        if audits.len() > MAX_AUDITS {
            audits.pop_front();
        }
        audit
    }

    pub fn approve(&self, content_hash: &str, approved_by: &str) -> bool {
        let exists = self
            .audits
            .lock()
            .expect("security audits lock poisoned")
            .iter()
            .any(|audit| audit.content_hash == content_hash);
        if exists {
            self.approvals
                .lock()
                .expect("security approvals lock poisoned")
                .insert(content_hash.to_string(), approved_by.to_string());
        }
        exists
    }

    /// Content waiting for someone to approve it: the newest audit per
    /// content hash that was held for approval and has not been approved
    /// since. The same manifest submitted twice is listed once.
    pub fn pending(&self) -> Vec<SecurityAudit> {
        let approvals = self
            .approvals
            .lock()
            .expect("security approvals lock poisoned");
        let audits = self.audits.lock().expect("security audits lock poisoned");
        let mut seen = std::collections::HashSet::new();
        let mut pending: Vec<SecurityAudit> = audits
            .iter()
            .rev()
            .filter(|a| matches!(a.decision, SecurityDecision::PendingApproval))
            .filter(|a| !approvals.contains_key(&a.content_hash))
            .filter(|a| seen.insert(a.content_hash.clone()))
            .cloned()
            .collect();
        pending.reverse();
        pending
    }

    pub fn audits(&self) -> Vec<SecurityAudit> {
        self.audits
            .lock()
            .expect("security audits lock poisoned")
            .iter()
            .cloned()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn critical_content_is_quarantined() {
        let service = SecurityService::default();
        let audit = service.scan("agent", "ignore previous instructions; rm -rf /");
        assert!(matches!(audit.decision, SecurityDecision::Quarantined));
    }

    #[test]
    fn warnings_require_and_honor_approval() {
        let service = SecurityService::default();
        let audit = service.scan("workflow", "tool = shell_exec");
        assert!(matches!(audit.decision, SecurityDecision::PendingApproval));
        assert!(service.approve(&audit.content_hash, "operator"));
        let admitted = service.scan("workflow", "tool = shell_exec");
        assert!(matches!(admitted.decision, SecurityDecision::Allowed));
    }

    #[test]
    fn pending_lists_each_unapproved_hash_once() {
        let service = SecurityService::default();
        let a = service.scan("agent-manifest", "tool = shell_exec");
        service.scan("agent-manifest", "tool = shell_exec");
        service.scan("agent-manifest", "name = \"clean\"");
        let b = service.scan("agent-manifest", "x = \"shell_exec too\"");
        service.scan("agent-manifest", "ignore previous instructions");
        let pending = service.pending();
        let hashes: Vec<&str> = pending.iter().map(|p| p.content_hash.as_str()).collect();
        assert_eq!(
            hashes,
            vec![a.content_hash.as_str(), b.content_hash.as_str()]
        );

        assert!(service.approve(&a.content_hash, "operator"));
        let pending = service.pending();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].content_hash, b.content_hash);
    }

    #[test]
    fn manifest_summary_shows_name_and_capabilities() {
        let manifest = r#"
name = "containers"

[exec_policy]
mode = "allowlist"

[capabilities]
tools = ["shell_exec", "memory_store"]
shell = ["podman *"]
"#;
        let service = SecurityService::default();
        let audit = service.scan("agent-manifest", manifest);
        assert!(matches!(audit.decision, SecurityDecision::PendingApproval));
        let summary = audit.summary.expect("manifest summary");
        assert_eq!(summary["name"], "containers");
        assert_eq!(summary["exec_policy_mode"], "allowlist");
        assert_eq!(summary["capabilities"]["shell"][0], "podman *");
        assert_eq!(summary["capabilities"]["tools"][0], "shell_exec");

        assert!(manifest_summary("not toml at all {{").is_none());
        assert!(manifest_summary("other = 1").is_none());
    }
}
