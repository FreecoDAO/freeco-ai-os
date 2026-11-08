//! Container management tools: see, read and restart the containers on this
//! machine without a shell.
//!
//! Agents had no working way to manage containers. `shell_exec` refuses
//! `podman` and `docker` (neither is in the default exec allowlist, and any
//! pipe is refused outright), and `sandbox_exec` runs inside a container that
//! has no container runtime at all. The hand tools only reach containers that
//! a hand started, so a service installed from the Services page, or one
//! started by hand, was invisible. An agent asked to "restart the CRM" could
//! only fail, or ask the owner to loosen the shell policy.
//!
//! These tools call the runtime directly, with fixed verbs and a validated
//! container name, so nothing an agent writes reaches a shell. The runtime is
//! whichever of Docker or Podman answers, found the same way the sandbox
//! finds it. Inside the Podman deployment the `podman` client reaches the VM's
//! Podman through its socket, so the daemon sees its sibling containers.
//!
//! Output is not scrubbed here: every tool result passes through the data
//! gateway before it reaches a model, which masks keys and tokens a log may
//! contain.

use std::time::Duration;

/// Reading the container list or a log should take a second; twenty covers a
/// runtime that is busy, and stops a wedged one from hanging the agent.
const READ_TIMEOUT: Duration = Duration::from_secs(20);

/// Stopping waits up to `STOP_GRACE_SECS` for a clean shutdown, then the
/// runtime kills the container. The timeout covers that wait plus a slow
/// start afterwards.
const ACTION_TIMEOUT: Duration = Duration::from_secs(90);
const STOP_GRACE_SECS: u32 = 20;

const DEFAULT_LOG_LINES: u64 = 100;
const MAX_LOG_LINES: u64 = 1000;
/// The newest part of a log is kept when it is longer than this.
const MAX_LOG_CHARS: usize = 20_000;

/// Containers no agent may stop or restart. `freeco-daemon` is FreEco itself
/// in the Podman deployment: stopping it ends the conversation that asked for
/// it, and nothing would be left running to start it again.
const DEFAULT_PROTECTED: &[&str] = &["freeco-daemon"];

/// Extra protected names, comma separated, for installs that name things
/// differently.
const PROTECTED_ENV: &str = "FREECO_PROTECTED_CONTAINERS";

/// One container, as the tools report it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContainerSummary {
    pub name: String,
    pub id: String,
    pub image: String,
    /// Machine state: `running`, `exited`, `created`, `paused`...
    pub state: String,
    /// The runtime's own sentence, e.g. "Up 3 hours (healthy)".
    pub status: String,
    pub ports: String,
}

/// What a mutating tool does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContainerAction {
    Start,
    Stop,
    Restart,
}

impl ContainerAction {
    fn verb(self) -> &'static str {
        match self {
            ContainerAction::Start => "start",
            ContainerAction::Stop => "stop",
            ContainerAction::Restart => "restart",
        }
    }

    fn past(self) -> &'static str {
        match self {
            ContainerAction::Start => "started",
            ContainerAction::Stop => "stopped",
            ContainerAction::Restart => "restarted",
        }
    }
}

/// Accept only what a container name or id can be. A leading `-` would be
/// read as a flag, so the name must start with a letter or digit.
pub fn validate_container_name(name: &str) -> Result<&str, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("name is required: the container's name or id, from container_list".into());
    }
    let mut chars = name.chars();
    let first_ok = chars.next().is_some_and(|c| c.is_ascii_alphanumeric());
    let rest_ok = chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'));
    if !first_ok || !rest_ok || name.len() > 128 {
        return Err(format!(
            "'{name}' is not a container name. Use a name or id exactly as container_list shows it."
        ));
    }
    Ok(name)
}

/// The protected names: the defaults plus any in `FREECO_PROTECTED_CONTAINERS`.
pub fn protected_containers() -> Vec<String> {
    let mut names: Vec<String> = DEFAULT_PROTECTED.iter().map(|s| s.to_string()).collect();
    if let Ok(extra) = std::env::var(PROTECTED_ENV) {
        names.extend(
            extra
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(String::from),
        );
    }
    names
}

fn is_protected(name: &str, protected: &[String]) -> bool {
    protected.iter().any(|p| p.eq_ignore_ascii_case(name))
}

/// Parse `ps -a --format json`.
///
/// Podman prints one JSON array; Docker prints one object per line. The field
/// shapes differ too (Podman's `Names` is a list and `Ports` a list of
/// objects; Docker gives both as strings), so both are accepted.
pub fn parse_ps_output(raw: &str) -> Result<Vec<ContainerSummary>, String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() || trimmed == "null" {
        return Ok(Vec::new());
    }
    let values: Vec<serde_json::Value> = if trimmed.starts_with('[') {
        serde_json::from_str(trimmed)
            .map_err(|e| format!("could not read the container list: {e}"))?
    } else {
        trimmed
            .lines()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(serde_json::from_str)
            .collect::<Result<_, _>>()
            .map_err(|e| format!("could not read the container list: {e}"))?
    };
    let mut out: Vec<ContainerSummary> = values.iter().filter_map(summary_from_value).collect();
    out.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(out)
}

fn summary_from_value(v: &serde_json::Value) -> Option<ContainerSummary> {
    let name = match v.get("Names") {
        Some(serde_json::Value::Array(names)) => names
            .iter()
            .filter_map(|n| n.as_str())
            .collect::<Vec<_>>()
            .join(","),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => String::new(),
    };
    let id = v
        .get("Id")
        .or_else(|| v.get("ID"))
        .and_then(|i| i.as_str())
        .unwrap_or_default();
    if name.is_empty() && id.is_empty() {
        return None;
    }
    let text = |key: &str| {
        v.get(key)
            .and_then(|s| s.as_str())
            .unwrap_or_default()
            .to_string()
    };
    Some(ContainerSummary {
        name: if name.is_empty() {
            id.chars().take(12).collect()
        } else {
            name
        },
        id: id.chars().take(12).collect(),
        image: text("Image"),
        state: text("State").to_lowercase(),
        status: text("Status"),
        ports: ports_text(v.get("Ports")),
    })
}

fn ports_text(ports: Option<&serde_json::Value>) -> String {
    match ports {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Array(list)) => list
            .iter()
            .filter_map(|p| {
                let container = p.get("container_port").and_then(|c| c.as_u64())?;
                let proto = p.get("protocol").and_then(|c| c.as_str()).unwrap_or("tcp");
                match p.get("host_port").and_then(|h| h.as_u64()) {
                    Some(host) if host > 0 => {
                        let ip = p
                            .get("host_ip")
                            .and_then(|h| h.as_str())
                            .filter(|h| !h.is_empty())
                            .unwrap_or("0.0.0.0");
                        Some(format!("{ip}:{host}->{container}/{proto}"))
                    }
                    _ => Some(format!("{container}/{proto}")),
                }
            })
            .collect::<Vec<_>>()
            .join(", "),
        _ => String::new(),
    }
}

/// Find the one container `wanted` names: an exact name, or an id prefix.
pub fn find_container<'a>(
    list: &'a [ContainerSummary],
    wanted: &str,
) -> Result<&'a ContainerSummary, String> {
    if let Some(c) = list.iter().find(|c| c.name.split(',').any(|n| n == wanted)) {
        return Ok(c);
    }
    let by_id: Vec<&ContainerSummary> = list
        .iter()
        .filter(|c| !c.id.is_empty() && (c.id.starts_with(wanted) || wanted.starts_with(&c.id)))
        .collect();
    match by_id.as_slice() {
        [one] => Ok(one),
        [] => {
            let names: Vec<&str> = list.iter().map(|c| c.name.as_str()).collect();
            Err(format!(
                "No container is called '{wanted}'. The containers here are: {}.",
                if names.is_empty() {
                    "none".to_string()
                } else {
                    names.join(", ")
                }
            ))
        }
        _ => Err(format!(
            "'{wanted}' matches more than one container id. Use the full name from container_list."
        )),
    }
}

/// Render the list for an agent: one line per container, running first.
pub fn render_list(
    runtime: &str,
    list: &[ContainerSummary],
    filter: Option<&str>,
    protected: &[String],
) -> String {
    let filter = filter
        .map(|f| f.trim().to_lowercase())
        .filter(|f| !f.is_empty());
    let shown: Vec<&ContainerSummary> = list
        .iter()
        .filter(|c| match &filter {
            Some(f) => c.name.to_lowercase().contains(f) || c.image.to_lowercase().contains(f),
            None => true,
        })
        .collect();
    let running = shown.iter().filter(|c| c.state == "running").count();
    let mut out = match &filter {
        Some(f) => format!(
            "Runtime: {runtime}. {} containers match '{f}', {running} running.\n",
            shown.len()
        ),
        None => format!(
            "Runtime: {runtime}. {} containers, {running} running.\n",
            shown.len()
        ),
    };
    let mut ordered = shown;
    ordered.sort_by_key(|c| (c.state != "running", c.name.clone()));
    for c in ordered {
        out.push_str(&format!(
            "- {} | {} | {} | {}",
            c.name, c.state, c.status, c.image
        ));
        if !c.ports.is_empty() {
            out.push_str(&format!(" | ports {}", c.ports));
        }
        if is_protected(&c.name, protected) {
            out.push_str(" | protected: FreEco itself, cannot be stopped from here");
        }
        out.push('\n');
    }
    out
}

/// Keep the newest `max_chars` of a log, cut at a line start.
pub fn tail_chars(text: &str, max_chars: usize) -> (String, bool) {
    let count = text.chars().count();
    if count <= max_chars {
        return (text.to_string(), false);
    }
    let skip = count - max_chars;
    let start = text.char_indices().nth(skip).map(|(i, _)| i).unwrap_or(0);
    let cut = &text[start..];
    let cut = match cut.find('\n') {
        Some(nl) if nl + 1 < cut.len() => &cut[nl + 1..],
        _ => cut,
    };
    (cut.to_string(), true)
}

async fn runtime() -> Result<&'static str, String> {
    crate::docker_sandbox::container_runtime()
        .await
        .ok_or_else(|| {
            "No container runtime answers here: neither `docker` nor `podman` responded. \
         On Windows, start Podman with `podman machine start`; elsewhere, start the Docker \
         or Podman service. Then try again."
                .to_string()
        })
}

async fn run(
    runtime: &str,
    args: &[&str],
    timeout: Duration,
) -> Result<std::process::Output, String> {
    let mut cmd = crate::quiet_command::quiet_async(runtime);
    cmd.args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .kill_on_drop(true);
    match tokio::time::timeout(timeout, cmd.output()).await {
        Ok(Ok(out)) => Ok(out),
        Ok(Err(e)) => Err(format!("could not run {runtime}: {e}")),
        Err(_) => Err(format!(
            "{runtime} {} did not finish within {} seconds",
            args.first().copied().unwrap_or_default(),
            timeout.as_secs()
        )),
    }
}

async fn list_containers(runtime: &str) -> Result<Vec<ContainerSummary>, String> {
    let out = run(runtime, &["ps", "-a", "--format", "json"], READ_TIMEOUT).await?;
    if !out.status.success() {
        return Err(format!(
            "{runtime} ps failed: {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    parse_ps_output(&String::from_utf8_lossy(&out.stdout))
}

/// `container_list`: every container the runtime knows, whatever started it.
pub async fn tool_container_list(input: &serde_json::Value) -> Result<String, String> {
    let rt = runtime().await?;
    let list = list_containers(rt).await?;
    Ok(render_list(
        rt,
        &list,
        input["filter"].as_str(),
        &protected_containers(),
    ))
}

/// `container_logs`: the last lines a container wrote.
pub async fn tool_container_logs(input: &serde_json::Value) -> Result<String, String> {
    let name = validate_container_name(input["name"].as_str().unwrap_or_default())?;
    let lines = input["lines"]
        .as_u64()
        .unwrap_or(DEFAULT_LOG_LINES)
        .clamp(1, MAX_LOG_LINES);
    let rt = runtime().await?;
    let tail = lines.to_string();
    let out = run(rt, &["logs", "--tail", &tail, name], READ_TIMEOUT).await?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    if !out.status.success() {
        // Most often a wrong name: say which names exist instead of passing
        // on the runtime's "no such container".
        if let Ok(list) = list_containers(rt).await {
            find_container(&list, name)?;
        }
        return Err(format!("{rt} logs {name} failed: {}", stderr.trim()));
    }
    // A container's stdout and stderr arrive on separate streams, so their
    // order relative to each other is lost; each is in order on its own.
    let body = match (stdout.trim().is_empty(), stderr.trim().is_empty()) {
        (true, true) => return Ok(format!("{name} has written nothing to its log.")),
        (false, true) => stdout.into_owned(),
        (true, false) => stderr.into_owned(),
        (false, false) => format!("[stdout]\n{stdout}\n[stderr]\n{stderr}"),
    };
    let (body, cut) = tail_chars(&body, MAX_LOG_CHARS);
    Ok(if cut {
        format!("Last lines of {name} (older lines cut to fit):\n{body}")
    } else {
        format!("Last {lines} lines of {name}:\n{body}")
    })
}

/// `container_start` / `container_stop` / `container_restart`.
///
/// The target is looked up in the container list first, so a protected
/// container is refused by its id as well as its name, and the agent is told
/// the container's state afterwards rather than only that the command ran.
pub async fn tool_container_action(
    action: ContainerAction,
    input: &serde_json::Value,
) -> Result<String, String> {
    let wanted = validate_container_name(input["name"].as_str().unwrap_or_default())?;
    let rt = runtime().await?;
    let list = list_containers(rt).await?;
    let target = find_container(&list, wanted)?.clone();
    let name = target.name.split(',').next().unwrap_or(wanted).to_string();

    if action != ContainerAction::Start
        && target
            .name
            .split(',')
            .any(|n| is_protected(n, &protected_containers()))
    {
        return Err(format!(
            "{name} is FreEco itself. Stopping or restarting it from here would end this \
             conversation with nothing left to start it again. Ask the owner to run \
             `podman restart {name}` on the machine instead."
        ));
    }
    if action == ContainerAction::Start && target.state == "running" {
        return Ok(format!("{name} is already running ({}).", target.status));
    }
    if action == ContainerAction::Stop && target.state != "running" {
        return Ok(format!("{name} is not running ({}).", target.status));
    }

    let grace = STOP_GRACE_SECS.to_string();
    // `-t`, not a long flag: Podman spells it `--time`, Docker `--timeout`
    // (its `--time` is deprecated), and both accept `-t`.
    let args: Vec<&str> = match action {
        ContainerAction::Start => vec!["start", &name],
        ContainerAction::Stop => vec!["stop", "-t", &grace, &name],
        ContainerAction::Restart => vec!["restart", "-t", &grace, &name],
    };
    let out = run(rt, &args, ACTION_TIMEOUT).await?;
    if !out.status.success() {
        return Err(format!(
            "{rt} {} {name} failed: {}. Read container_logs {name} for the cause.",
            action.verb(),
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }

    // Report the state the runtime now shows: a container that exits a
    // second after starting looks "started" to the command alone.
    let after = list_containers(rt)
        .await
        .ok()
        .and_then(|l| find_container(&l, &name).ok().cloned());
    Ok(match after {
        Some(c) if action != ContainerAction::Stop && c.state != "running" => format!(
            "{name} was {} but is not running now ({}). Read container_logs {name} for the cause.",
            action.past(),
            c.status
        ),
        Some(c) => format!("{name} {}: now {} ({}).", action.past(), c.state, c.status),
        None => format!("{name} {}.", action.past()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const PODMAN_PS: &str = r#"[
      {"Id":"4f1c2b3a9d8e7f6a5b4c3d2e1f0a9b8c","Names":["freeco-speaches"],"Image":"ghcr.io/speaches-ai/speaches:latest","State":"exited","Status":"Exited (1) 2 minutes ago","Ports":[{"host_ip":"127.0.0.1","container_port":8000,"host_port":8000,"range":1,"protocol":"tcp"}]},
      {"Id":"9a8b7c6d5e4f3a2b1c0d9e8f7a6b5c4d","Names":["freeco-daemon"],"Image":"localhost/freeco-daemon:latest","State":"running","Status":"Up 3 hours","Ports":null},
      {"Id":"1234567890abcdef1234567890abcdef","Names":["twenty"],"Image":"twentycrm/twenty:latest","State":"running","Status":"Up 3 hours (healthy)","Ports":[{"host_ip":"","container_port":3000,"host_port":3200,"range":1,"protocol":"tcp"}]}
    ]"#;

    const DOCKER_PS: &str = "{\"ID\":\"abc123def456\",\"Names\":\"crm-db\",\"Image\":\"postgres:16\",\"State\":\"running\",\"Status\":\"Up 5 minutes\",\"Ports\":\"5432/tcp\"}\n{\"ID\":\"0123456789ab\",\"Names\":\"crm\",\"Image\":\"twentycrm/twenty:latest\",\"State\":\"exited\",\"Status\":\"Exited (0) 1 hour ago\",\"Ports\":\"\"}\n";

    #[test]
    fn podman_json_array_is_read() {
        let list = parse_ps_output(PODMAN_PS).unwrap();
        assert_eq!(list.len(), 3);
        // Sorted by name.
        assert_eq!(list[0].name, "freeco-daemon");
        assert_eq!(list[1].name, "freeco-speaches");
        assert_eq!(list[1].id, "4f1c2b3a9d8e");
        assert_eq!(list[1].state, "exited");
        assert_eq!(list[1].ports, "127.0.0.1:8000->8000/tcp");
        assert_eq!(list[2].ports, "0.0.0.0:3200->3000/tcp");
        assert_eq!(list[0].ports, "");
    }

    #[test]
    fn docker_json_lines_are_read() {
        let list = parse_ps_output(DOCKER_PS).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].name, "crm");
        assert_eq!(list[0].state, "exited");
        assert_eq!(list[1].name, "crm-db");
        assert_eq!(list[1].ports, "5432/tcp");
    }

    #[test]
    fn an_empty_runtime_is_an_empty_list() {
        assert!(parse_ps_output("").unwrap().is_empty());
        assert!(parse_ps_output("[]").unwrap().is_empty());
        assert!(parse_ps_output("null").unwrap().is_empty());
    }

    #[test]
    fn garbage_is_an_error_not_an_empty_list() {
        assert!(parse_ps_output("permission denied").is_err());
    }

    #[test]
    fn names_that_could_be_flags_or_shell_are_refused() {
        for bad in [
            "",
            "-rf",
            "--all",
            "a b",
            "x;rm -rf /",
            "$(id)",
            "a|b",
            "../etc",
            "name\n",
        ] {
            assert!(
                validate_container_name(bad).is_err() || bad.trim() != bad,
                "{bad:?} must be refused"
            );
        }
        assert!(validate_container_name("-rf").is_err());
        assert!(validate_container_name("x;rm").is_err());
        assert_eq!(validate_container_name(" twenty ").unwrap(), "twenty");
        assert!(validate_container_name("freeco-speaches_1.2").is_ok());
        assert!(validate_container_name(&"a".repeat(129)).is_err());
    }

    #[test]
    fn containers_are_found_by_name_or_unique_id_prefix() {
        let list = parse_ps_output(PODMAN_PS).unwrap();
        assert_eq!(find_container(&list, "twenty").unwrap().name, "twenty");
        assert_eq!(
            find_container(&list, "4f1c2b").unwrap().name,
            "freeco-speaches"
        );
        // Full 64-char style ids still match the short id kept.
        assert_eq!(
            find_container(&list, "4f1c2b3a9d8e7f6a5b4c").unwrap().name,
            "freeco-speaches"
        );
        let err = find_container(&list, "crm").unwrap_err();
        assert!(err.contains("freeco-speaches"), "lists real names: {err}");
    }

    #[test]
    fn an_ambiguous_id_prefix_is_refused() {
        let list = vec![
            ContainerSummary {
                name: "a".into(),
                id: "abc111".into(),
                image: String::new(),
                state: "running".into(),
                status: String::new(),
                ports: String::new(),
            },
            ContainerSummary {
                name: "b".into(),
                id: "abc222".into(),
                image: String::new(),
                state: "running".into(),
                status: String::new(),
                ports: String::new(),
            },
        ];
        assert!(find_container(&list, "abc")
            .unwrap_err()
            .contains("more than one"));
    }

    #[test]
    fn the_list_puts_running_first_and_marks_freeco_itself() {
        let list = parse_ps_output(PODMAN_PS).unwrap();
        let text = render_list("podman", &list, None, &protected_containers());
        assert!(text.starts_with("Runtime: podman. 3 containers, 2 running."));
        let lines: Vec<&str> = text.lines().skip(1).collect();
        assert!(lines[0].starts_with("- freeco-daemon | running"));
        assert!(lines[0].contains("protected"));
        assert!(lines[1].starts_with("- twenty | running"));
        assert!(lines[2].starts_with("- freeco-speaches | exited"));
        assert!(!lines[2].contains("protected"));
    }

    #[test]
    fn the_list_filters_by_name_or_image() {
        let list = parse_ps_output(PODMAN_PS).unwrap();
        let text = render_list("podman", &list, Some("SPEACHES"), &[]);
        assert!(text.contains("1 containers match 'speaches', 0 running"));
        assert!(text.contains("freeco-speaches"));
        assert!(!text.contains("twenty"));
        let by_image = render_list("podman", &list, Some("twentycrm"), &[]);
        assert!(by_image.contains("- twenty |"));
    }

    #[test]
    fn freeco_itself_is_protected_by_default() {
        let protected = protected_containers();
        assert!(is_protected("freeco-daemon", &protected));
        assert!(is_protected("FreEco-Daemon", &protected));
        assert!(!is_protected("twenty", &protected));
    }

    #[test]
    fn long_logs_keep_the_newest_whole_lines() {
        let log: String = (0..1000).map(|i| format!("line {i}\n")).collect();
        let (kept, cut) = tail_chars(&log, 100);
        assert!(cut);
        assert!(kept.chars().count() <= 100);
        assert!(kept.ends_with("line 999\n"));
        assert!(
            kept.starts_with("line "),
            "starts on a whole line: {kept:?}"
        );
        let (short, cut) = tail_chars("one\ntwo\n", 100);
        assert!(!cut);
        assert_eq!(short, "one\ntwo\n");
    }

    #[test]
    fn tail_is_safe_on_multibyte_text() {
        let log = "ü".repeat(50) + "\nend\n";
        let (kept, cut) = tail_chars(&log, 10);
        assert!(cut);
        assert_eq!(kept, "end\n");
    }
}
