//! Container limits for `shell_exec` and `process_start`.
//!
//! An agent or hand allowed to run `podman` (or `docker`) with any arguments
//! can reach the host in one command: `run --privileged`, `-v /:/host`,
//! `--network host`, `--pid host`, `--cap-add`, or a port published on every
//! interface. The `[exec_policy]` allowlist only sees the binary name, so
//! once `podman` is allowed all of that is allowed too.
//!
//! When an exec policy carries `[exec_policy.containers]`, every container
//! command is parsed here before it runs:
//!
//! - container names (and volume names) must start with one of
//!   `name_prefixes`; `run`/`create` must pass `--name`;
//! - images must match one of `images` (`*` matches any run of characters);
//! - the host-escape flags are refused whatever the lists say;
//! - subcommands that this parser does not understand are refused rather
//!   than guessed at (`build`, `compose`, `kube play`, `cp`, `prune`, ...).
//!
//! The check is a pure function of the argv and the policy, so it is the
//! same in allowlist mode (direct exec) and full mode (`sh -c`). In full mode
//! the shell would expand `$VAR`, so any `$` in a container command is
//! refused: the kernel must see the value that will actually run.

use freeco_types::config::ContainerPolicy;

/// Binaries whose commands are checked.
const CONTAINER_BINARIES: &[&str] = &["podman", "docker", "podman-remote", "nerdctl"];

/// Binaries that run a whole file of containers the kernel cannot see.
const COMPOSE_BINARIES: &[&str] = &["podman-compose", "docker-compose"];

/// How deep `sh -c "podman ..."` style nesting is followed.
const MAX_NESTING: usize = 3;

/// Check a shell command string. Commands that do not run a container
/// binary pass unchanged.
pub fn check_command(
    command: &str,
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    check_command_at(command, policy, protected, 0)
}

/// Check an argv (for `process_start`, which never goes through a shell).
pub fn check_argv(
    argv: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    check_argv_at(argv, policy, protected, 0)
}

fn check_command_at(
    command: &str,
    policy: &ContainerPolicy,
    protected: &[String],
    depth: usize,
) -> Result<(), String> {
    let argv = shlex::split(command)
        .ok_or_else(|| "container check: the command has unmatched quotes".to_string())?;
    check_argv_at(&argv, policy, protected, depth)
}

fn check_argv_at(
    argv: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
    depth: usize,
) -> Result<(), String> {
    for (i, token) in argv.iter().enumerate() {
        // A quoted script (`sh -c "podman run ..."`) arrives as one token.
        if depth < MAX_NESTING && token.split_whitespace().nth(1).is_some() {
            check_command_at(token, policy, protected, depth + 1)?;
        }
        let base = binary_name(token);
        if COMPOSE_BINARIES.contains(&base.as_str()) {
            return Err(refusal(
                "compose files are not allowed: the kernel cannot check the containers they \
                 start. Run each container with `podman run`.",
            ));
        }
        if CONTAINER_BINARIES.contains(&base.as_str()) {
            return check_invocation(&argv[i + 1..], policy, protected);
        }
    }
    Ok(())
}

/// `/usr/bin/podman`, `podman.exe` and `PODMAN` all name `podman`.
fn binary_name(token: &str) -> String {
    let last = token.rsplit(['/', '\\']).next().unwrap_or(token);
    let lower = last.to_ascii_lowercase();
    lower.strip_suffix(".exe").unwrap_or(&lower).to_string()
}

fn refusal(reason: &str) -> String {
    format!("Container command refused by exec_policy.containers: {reason}")
}

/// Check everything after the container binary.
fn check_invocation(
    args: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    if let Some(arg) = args.iter().find(|a| a.contains('$')) {
        return Err(refusal(&format!(
            "'{arg}' contains '$'. Variables are not allowed in container commands: write \
             the value itself so the kernel can check it."
        )));
    }
    let Some(first) = args.first() else {
        return Ok(()); // bare `podman` prints help
    };
    if first.starts_with('-') {
        return match first.as_str() {
            "--version" | "-v" | "--help" | "-h" if args.len() == 1 => Ok(()),
            _ => Err(refusal(&format!(
                "global option '{first}' before the subcommand is not allowed (it can point \
                 podman at another machine or storage). Put options after the subcommand."
            ))),
        };
    }
    let sub = first.as_str();
    let rest = &args[1..];
    match sub {
        "container" => check_container_namespace(rest, policy, protected),
        "image" => check_image_namespace(rest, policy),
        "volume" => check_volume(rest, policy),
        "network" => check_network(rest),
        "system" => match rest.first().map(String::as_str) {
            Some("df") | Some("info") => Ok(()),
            other => Err(unsupported(&format!("system {}", other.unwrap_or("")))),
        },
        "healthcheck" => match rest.first().map(String::as_str) {
            Some("run") => check_targets("healthcheck", &rest[1..], policy, protected),
            _ => Err(unsupported("healthcheck")),
        },
        _ => check_container_sub(sub, rest, policy, protected),
    }
}

fn unsupported(sub: &str) -> String {
    refusal(&format!(
        "'{}' is not a container subcommand the kernel can check, so it is refused. Allowed: \
         ps, images, inspect, logs, port, top, stats, diff, run, create, start, stop, restart, \
         kill, rm, pause, unpause, wait, rename, exec, commit, pull, push, tag, rmi, \
         volume ls/inspect/create/rm, network ls/inspect, info, version.",
        sub.trim()
    ))
}

/// `podman container <sub>` is `podman <sub>`.
fn check_container_namespace(
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    let Some(sub) = rest.first() else {
        return Ok(());
    };
    let sub = match sub.as_str() {
        "ls" | "list" => "ps",
        other => other,
    };
    check_container_sub(sub, &rest[1..], policy, protected)
}

/// `podman image <sub>`.
fn check_image_namespace(rest: &[String], policy: &ContainerPolicy) -> Result<(), String> {
    let Some(sub) = rest.first() else {
        return Ok(());
    };
    let args = &rest[1..];
    match sub.as_str() {
        "ls" | "list" | "exists" | "history" => Ok(()),
        "inspect" => Ok(()),
        "pull" => check_images_positional("pull", args, policy, PULL_VALUE_FLAGS),
        "push" => check_images_positional("push", args, policy, PULL_VALUE_FLAGS),
        "tag" => check_images_positional("tag", args, policy, &[]),
        "rm" => check_rmi(args, policy),
        other => Err(unsupported(&format!("image {other}"))),
    }
}

fn check_container_sub(
    sub: &str,
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    match sub {
        // Read-only, no single target.
        "ps" | "images" | "version" | "info" => Ok(()),
        "run" | "create" => check_run(sub, rest, policy, protected),
        "exec" => check_exec(rest, policy, protected),
        "pull" => check_images_positional("pull", rest, policy, PULL_VALUE_FLAGS),
        "push" => check_images_positional("push", rest, policy, PULL_VALUE_FLAGS),
        "tag" => check_images_positional("tag", rest, policy, &[]),
        "rmi" => check_rmi(rest, policy),
        "commit" => check_commit(rest, policy, protected),
        "inspect" | "logs" | "port" | "top" | "stats" | "diff" | "start" | "stop" | "restart"
        | "kill" | "rm" | "pause" | "unpause" | "wait" | "rename" => {
            check_targets(sub, rest, policy, protected)
        }
        other => Err(unsupported(other)),
    }
}

/// Subcommands that change a container (the protected ones may not be
/// targeted by these).
fn is_mutating(sub: &str) -> bool {
    !matches!(
        sub,
        "inspect" | "logs" | "port" | "top" | "stats" | "diff" | "wait" | "healthcheck"
    )
}

/// Flags that take a value, per target subcommand. Anything unknown is read
/// as a boolean; if it really took a value, that value is then checked as a
/// container name, which can only make the check stricter.
fn target_value_flags(sub: &str) -> &'static [&'static str] {
    match sub {
        "inspect" => &["-f", "--format", "-t", "--type"],
        "logs" => &["--tail", "--since", "--until"],
        "stop" | "restart" | "rm" => &["-t", "--time", "--timeout"],
        "kill" => &["-s", "--signal"],
        "stats" => &["--format", "-i", "--interval"],
        "wait" => &["--condition", "-i", "--interval"],
        "start" => &["--detach-keys"],
        "diff" => &["--format"],
        _ => &[],
    }
}

/// Split `--flag=value` into its parts.
fn split_flag(arg: &str) -> (&str, Option<&str>) {
    match arg.split_once('=') {
        Some((f, v)) if arg.starts_with("--") => (f, Some(v)),
        _ => (arg, None),
    }
}

fn name_in_scope(name: &str, policy: &ContainerPolicy) -> bool {
    policy.name_prefixes.is_empty() || policy.name_prefixes.iter().any(|p| name.starts_with(p))
}

fn is_protected(name: &str, protected: &[String]) -> bool {
    protected.iter().any(|p| p.eq_ignore_ascii_case(name))
}

fn out_of_scope(kind: &str, name: &str, policy: &ContainerPolicy) -> String {
    refusal(&format!(
        "{kind} '{name}' is outside this agent's scope: names must start with {}.",
        policy
            .name_prefixes
            .iter()
            .map(|p| format!("'{p}'"))
            .collect::<Vec<_>>()
            .join(" or ")
    ))
}

fn check_name(
    kind: &str,
    name: &str,
    policy: &ContainerPolicy,
    protected: &[String],
    mutating: bool,
) -> Result<(), String> {
    if !name_in_scope(name, policy) {
        return Err(out_of_scope(kind, name, policy));
    }
    if mutating && is_protected(name, protected) {
        return Err(refusal(&format!(
            "'{name}' is protected: it runs FreEco itself (or is listed in \
             FREECO_PROTECTED_CONTAINERS). Ask the owner to do this by hand."
        )));
    }
    Ok(())
}

/// Commands whose positionals are container names: start, stop, logs, ...
fn check_targets(
    sub: &str,
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    let mutating = is_mutating(sub);
    let value_flags = target_value_flags(sub);
    let mut positionals: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if arg == "--" {
            positionals.extend(rest[i + 1..].iter().map(String::as_str));
            break;
        }
        if arg.starts_with('-') && arg.len() > 1 {
            let (flag, inline) = split_flag(arg);
            match flag {
                "--latest" | "-l" => {
                    return Err(refusal(
                        "--latest picks whichever container was created last; name the \
                         container instead.",
                    ));
                }
                "--all" | "-a" if sub != "stats" && sub != "start" => {
                    return Err(refusal(&format!(
                        "'{sub} --all' would act on every container, including ones outside \
                         this agent's scope; name the containers instead."
                    )));
                }
                "--filter" | "--cidfile" | "--depend" | "--pod" if mutating => {
                    return Err(refusal(&format!(
                        "'{flag}' selects containers the kernel cannot check; name them instead."
                    )));
                }
                _ => {}
            }
            if inline.is_none() && value_flags.contains(&flag) {
                i += 1; // skip the value
            }
            i += 1;
            continue;
        }
        positionals.push(arg);
        i += 1;
    }
    // `top CONTAINER [descriptors...]`: only the first is a container.
    let targets: &[&str] = if sub == "top" {
        &positionals[..positionals.len().min(1)]
    } else {
        &positionals[..]
    };
    if targets.is_empty() && mutating {
        return Err(refusal(&format!("'{sub}' needs the container's name.")));
    }
    for name in targets {
        // `inspect` also takes image names.
        if sub == "inspect" && !name_in_scope(name, policy) && image_allowed(name, policy) {
            continue;
        }
        check_name("container", name, policy, protected, mutating)?;
    }
    Ok(())
}

fn image_allowed(image: &str, policy: &ContainerPolicy) -> bool {
    policy.images.is_empty()
        || policy
            .images
            .iter()
            .any(|p| wildcard_match(&p.to_ascii_lowercase(), &image.to_ascii_lowercase()))
}

fn check_image(image: &str, policy: &ContainerPolicy) -> Result<(), String> {
    if image_allowed(image, policy) {
        Ok(())
    } else {
        Err(refusal(&format!(
            "image '{image}' is not allowed. Allowed images: {}.",
            policy.images.join(", ")
        )))
    }
}

/// `*` matches any run of characters (including `/` and `:`).
fn wildcard_match(pattern: &str, value: &str) -> bool {
    let p: Vec<char> = pattern.chars().collect();
    let v: Vec<char> = value.chars().collect();
    let (mut pi, mut vi) = (0usize, 0usize);
    let mut star: Option<usize> = None;
    let mut mark = 0usize;
    while vi < v.len() {
        if pi < p.len() && p[pi] == '*' {
            star = Some(pi);
            mark = vi;
            pi += 1;
        } else if pi < p.len() && p[pi] == v[vi] {
            pi += 1;
            vi += 1;
        } else if let Some(s) = star {
            pi = s + 1;
            mark += 1;
            vi = mark;
        } else {
            return false;
        }
    }
    while pi < p.len() && p[pi] == '*' {
        pi += 1;
    }
    pi == p.len()
}

const PULL_VALUE_FLAGS: &[&str] = &[
    "--platform",
    "--arch",
    "--os",
    "--variant",
    "--creds",
    "--authfile",
    "--cert-dir",
    "--decryption-key",
    "--policy",
    "--retry",
    "--retry-delay",
    "--format",
    "--sign-by",
    "--digestfile",
    "--compression-format",
];

/// pull / push / tag: every positional is an image.
fn check_images_positional(
    sub: &str,
    rest: &[String],
    policy: &ContainerPolicy,
    value_flags: &[&str],
) -> Result<(), String> {
    let mut seen = 0;
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if arg.starts_with('-') && arg.len() > 1 {
            let (flag, inline) = split_flag(arg);
            if inline.is_none() && value_flags.contains(&flag) {
                i += 1;
            }
            i += 1;
            continue;
        }
        check_image(arg, policy)?;
        seen += 1;
        i += 1;
    }
    if seen == 0 {
        return Err(refusal(&format!("'{sub}' needs an image name.")));
    }
    Ok(())
}

fn check_rmi(rest: &[String], policy: &ContainerPolicy) -> Result<(), String> {
    for arg in rest {
        match split_flag(arg).0 {
            "--all" | "-a" => {
                return Err(refusal(
                    "'rmi --all' would remove every image; name the images instead.",
                ));
            }
            f if f.starts_with('-') => {}
            image => check_image(image, policy)?,
        }
    }
    Ok(())
}

/// `commit [opts] CONTAINER [IMAGE]`.
fn check_commit(
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    const VALUE: &[&str] = &[
        "-a",
        "--author",
        "-m",
        "--message",
        "-c",
        "--change",
        "-f",
        "--format",
        "--iidfile",
        "--config",
    ];
    let mut positionals: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if arg.starts_with('-') && arg.len() > 1 {
            let (flag, inline) = split_flag(arg);
            if inline.is_none() && VALUE.contains(&flag) {
                i += 1;
            }
            i += 1;
            continue;
        }
        positionals.push(arg);
        i += 1;
    }
    let Some(container) = positionals.first() else {
        return Err(refusal("'commit' needs the container's name."));
    };
    // A snapshot of FreEco itself would hold its secrets.
    check_name("container", container, policy, protected, true)?;
    if let Some(image) = positionals.get(1) {
        check_image(image, policy)?;
    }
    Ok(())
}

/// `volume ls|inspect|create|rm`.
fn check_volume(rest: &[String], policy: &ContainerPolicy) -> Result<(), String> {
    let Some(sub) = rest.first() else {
        return Ok(());
    };
    let args = &rest[1..];
    match sub.as_str() {
        "ls" | "list" | "inspect" | "exists" => Ok(()),
        "create" | "rm" | "remove" => {
            let mut i = 0;
            let mut named = 0;
            while i < args.len() {
                let arg = args[i].as_str();
                if arg.starts_with('-') && arg.len() > 1 {
                    let (flag, inline) = split_flag(arg);
                    match flag {
                        // `-o device=/srv,o=bind` turns a volume into a host mount.
                        "-o" | "--opt" => {
                            return Err(refusal(
                                "volume options (-o/--opt) can bind a host directory; they \
                                 are not allowed.",
                            ));
                        }
                        "-d" | "--driver" => {
                            let value = match inline {
                                Some(v) => Some(v),
                                None => {
                                    i += 1;
                                    args.get(i).map(String::as_str)
                                }
                            };
                            if value != Some("local") {
                                return Err(refusal(
                                    "only the 'local' volume driver is allowed.",
                                ));
                            }
                        }
                        "-a" | "--all" => {
                            return Err(refusal(
                                "'volume rm --all' would remove every volume; name them instead.",
                            ));
                        }
                        "-l" | "--label" | "-t" | "--time" if inline.is_none() => i += 1,
                        _ => {}
                    }
                    i += 1;
                    continue;
                }
                if !name_in_scope(arg, policy) {
                    return Err(out_of_scope("volume", arg, policy));
                }
                named += 1;
                i += 1;
            }
            if named == 0 {
                return Err(refusal("name the volume; generated names are outside the scope."));
            }
            Ok(())
        }
        other => Err(unsupported(&format!("volume {other}"))),
    }
}

fn check_network(rest: &[String]) -> Result<(), String> {
    match rest.first().map(String::as_str) {
        None | Some("ls") | Some("list") | Some("inspect") | Some("exists") => Ok(()),
        Some(other) => Err(unsupported(&format!("network {other}"))),
    }
}

/// A named volume, as opposed to a host path (`/srv`, `./data`, `~/x`, `C:\x`).
fn is_named_volume(src: &str) -> bool {
    let mut chars = src.chars();
    chars.next().is_some_and(|c| c.is_ascii_alphanumeric())
        && chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '.' | '-'))
}

fn check_volume_source(src: &str, policy: &ContainerPolicy) -> Result<(), String> {
    if !is_named_volume(src) {
        return Err(refusal(&format!(
            "'{src}' is a host path. Host directories cannot be mounted; use a named volume \
             (for example -v freeco-app-data:/data)."
        )));
    }
    if !name_in_scope(src, policy) {
        return Err(out_of_scope("volume", src, policy));
    }
    Ok(())
}

/// `-v SRC:DST[:OPTS]` or `-v DST` (anonymous volume).
fn check_volume_flag(value: &str, policy: &ContainerPolicy) -> Result<(), String> {
    if value.contains('\\') {
        return Err(refusal(&format!(
            "'{value}' looks like a host path; use a named volume."
        )));
    }
    let parts: Vec<&str> = value.split(':').collect();
    if parts.len() == 1 {
        // Anonymous volume at a container path. A bare name is a volume too.
        return if parts[0].starts_with('/') {
            Ok(())
        } else {
            check_volume_source(parts[0], policy)
        };
    }
    check_volume_source(parts[0], policy)
}

/// `--mount type=volume,source=x,target=/y`.
fn check_mount_flag(value: &str, policy: &ContainerPolicy) -> Result<(), String> {
    let mut kind: Option<&str> = None;
    let mut source: Option<&str> = None;
    for part in value.split(',') {
        let (k, v) = part.split_once('=').unwrap_or((part, ""));
        match k.trim() {
            "type" => kind = Some(v.trim()),
            "source" | "src" => source = Some(v.trim()),
            _ => {}
        }
    }
    match kind {
        Some("tmpfs") => Ok(()),
        Some("volume") => match source {
            Some(src) if !src.is_empty() => check_volume_source(src, policy),
            _ => Ok(()),
        },
        Some(other) => Err(refusal(&format!(
            "--mount type={other} is not allowed; only type=volume (named) and type=tmpfs."
        ))),
        None => Err(refusal(
            "--mount needs type=volume or type=tmpfs; bind mounts of host paths are not allowed.",
        )),
    }
}

/// `-p [IP:][HOSTPORT:]CONTAINERPORT[/PROTO]`: the host side must be loopback.
fn check_publish(value: &str) -> Result<(), String> {
    let ip: Option<&str> = if let Some(rest) = value.strip_prefix('[') {
        rest.split_once(']').map(|(ip, _)| ip)
    } else {
        let parts: Vec<&str> = value.split(':').collect();
        if parts.len() >= 3 {
            Some(parts[0])
        } else {
            None
        }
    };
    let loopback = match ip {
        Some(ip) => {
            let ip = ip.trim();
            ip == "localhost" || ip == "::1" || ip.starts_with("127.")
        }
        None => false,
    };
    if loopback {
        Ok(())
    } else {
        Err(refusal(&format!(
            "'-p {value}' publishes on every interface. Bind to loopback: \
             -p 127.0.0.1:HOSTPORT:CONTAINERPORT."
        )))
    }
}

/// `--pid`, `--ipc`, `--uts`, `--userns`, `--cgroupns`, `--network`.
fn check_namespace(
    flag: &str,
    value: &str,
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    let v = value.trim();
    let is_network = flag == "--network" || flag == "--net";
    if v == "host" {
        if is_network && policy.allow_host_network {
            return Ok(());
        }
        return Err(refusal(&format!(
            "'{flag} host' shares the host's namespace{}.",
            if is_network {
                " (allow_host_network is off)"
            } else {
                ""
            }
        )));
    }
    if v.starts_with("ns:") {
        return Err(refusal(&format!(
            "'{flag} {v}' joins a namespace by host path; not allowed."
        )));
    }
    if let Some(other) = v.strip_prefix("container:") {
        return check_name("container", other, policy, protected, false);
    }
    Ok(())
}

/// `run` / `create`.
fn check_run(
    sub: &str,
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    const LONG_VALUE: &[&str] = &[
        "--name",
        "--env",
        "--env-file",
        "--label",
        "--label-file",
        "--annotation",
        "--publish",
        "--volume",
        "--mount",
        "--network",
        "--net",
        "--network-alias",
        "--workdir",
        "--user",
        "--hostname",
        "--memory",
        "--memory-swap",
        "--memory-reservation",
        "--cpus",
        "--cpu-shares",
        "--cpuset-cpus",
        "--pids-limit",
        "--restart",
        "--entrypoint",
        "--health-cmd",
        "--health-interval",
        "--health-retries",
        "--health-timeout",
        "--health-start-period",
        "--health-on-failure",
        "--log-driver",
        "--log-opt",
        "--tz",
        "--pull",
        "--platform",
        "--arch",
        "--os",
        "--stop-signal",
        "--stop-timeout",
        "--add-host",
        "--dns",
        "--dns-search",
        "--dns-option",
        "--expose",
        "--pod",
        "--ip",
        "--ip6",
        "--mac-address",
        "--shm-size",
        "--ulimit",
        "--tmpfs",
        "--userns",
        "--pid",
        "--ipc",
        "--uts",
        "--cgroupns",
        "--security-opt",
        "--cap-add",
        "--cap-drop",
        "--device",
        "--volumes-from",
        "--secret",
        "--sdnotify",
        "--timeout",
        "--group-add",
        "--attach",
        "--detach-keys",
        "--init-path",
        "--cidfile",
        "--umask",
        "--requires",
        "--systemd",
        "--sysctl",
    ];
    const LONG_BOOL: &[&str] = &[
        "--detach",
        "--interactive",
        "--tty",
        "--rm",
        "--replace",
        "--init",
        "--read-only",
        "--read-only-tmpfs",
        "--quiet",
        "--no-healthcheck",
        "--no-hosts",
        "--http-proxy",
        "--sig-proxy",
        "--help",
    ];
    // Short flags: value-taking, and booleans that may be clustered (-dit).
    fn short_long(c: char) -> Option<&'static str> {
        Some(match c {
            'e' => "--env",
            'p' => "--publish",
            'v' => "--volume",
            'w' => "--workdir",
            'u' => "--user",
            'h' => "--hostname",
            'l' => "--label",
            'm' => "--memory",
            'c' => "--cpu-shares",
            'a' => "--attach",
            _ => return None,
        })
    }
    const SHORT_BOOL: &[char] = &['d', 'i', 't', 'q'];

    let mut name: Option<&str> = None;
    let mut image: Option<&str> = None;
    // (flag, value) pairs collected for checking.
    let mut opts: Vec<(String, String)> = Vec::new();
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if arg == "--" {
            image = rest.get(i + 1).map(String::as_str);
            break;
        }
        if !arg.starts_with('-') || arg.len() == 1 {
            image = Some(arg);
            break;
        }
        if arg.starts_with("--") {
            let (flag, inline) = split_flag(arg);
            match flag {
                "--privileged" => {
                    return Err(refusal("--privileged gives the container the whole host."));
                }
                "--publish-all" => {
                    return Err(refusal(
                        "--publish-all publishes every port on every interface; use \
                         -p 127.0.0.1:HOSTPORT:CONTAINERPORT.",
                    ));
                }
                "--rootfs" => {
                    return Err(refusal("--rootfs runs a host directory as the container."));
                }
                _ => {}
            }
            if LONG_BOOL.contains(&flag) {
                i += 1;
                continue;
            }
            if LONG_VALUE.contains(&flag) {
                let value = match inline {
                    Some(v) => v.to_string(),
                    None => {
                        i += 1;
                        match rest.get(i) {
                            Some(v) => v.clone(),
                            None => return Err(refusal(&format!("'{flag}' needs a value."))),
                        }
                    }
                };
                opts.push((flag.to_string(), value));
                i += 1;
                continue;
            }
            return Err(refusal(&format!(
                "'{flag}' is not an option the kernel knows for '{sub}', so it cannot tell \
                 what it does; it is refused."
            )));
        }
        // Short flag or cluster.
        let chars: Vec<char> = arg[1..].chars().collect();
        let mut j = 0;
        let mut consumed_next = false;
        while j < chars.len() {
            let c = chars[j];
            if c == 'P' {
                return Err(refusal(
                    "-P publishes every port on every interface; use \
                     -p 127.0.0.1:HOSTPORT:CONTAINERPORT.",
                ));
            }
            if SHORT_BOOL.contains(&c) {
                j += 1;
                continue;
            }
            if let Some(long) = short_long(c) {
                let attached: String = chars[j + 1..].iter().collect();
                let attached = attached.strip_prefix('=').unwrap_or(&attached).to_string();
                let value = if !attached.is_empty() {
                    attached
                } else {
                    consumed_next = true;
                    match rest.get(i + 1) {
                        Some(v) => v.clone(),
                        None => return Err(refusal(&format!("'-{c}' needs a value."))),
                    }
                };
                opts.push((long.to_string(), value));
                break;
            }
            return Err(refusal(&format!(
                "'-{c}' is not an option the kernel knows for '{sub}'; it is refused."
            )));
        }
        i += if consumed_next { 2 } else { 1 };
    }

    for (flag, value) in &opts {
        match flag.as_str() {
            "--name" => name = Some(value.as_str()),
            "--cap-add" => {
                return Err(refusal(&format!(
                    "--cap-add {value} adds kernel capabilities; not allowed."
                )));
            }
            "--device" => {
                return Err(refusal(&format!(
                    "--device {value} passes a host device in; not allowed."
                )));
            }
            "--volumes-from" => {
                return Err(refusal(
                    "--volumes-from copies another container's mounts; mount named volumes \
                     instead.",
                ));
            }
            "--security-opt" => {
                if !value.starts_with("no-new-privileges") {
                    return Err(refusal(&format!(
                        "--security-opt {value} weakens isolation; only no-new-privileges is \
                         allowed."
                    )));
                }
            }
            "--group-add" if value == "keep-groups" => {
                return Err(refusal("--group-add keep-groups passes the host's groups in."));
            }
            "--pid" | "--ipc" | "--uts" | "--userns" | "--cgroupns" | "--network" | "--net" => {
                check_namespace(flag, value, policy, protected)?;
            }
            "--pod" => {
                if !name_in_scope(value, policy) {
                    return Err(out_of_scope("pod", value, policy));
                }
            }
            "--volume" => check_volume_flag(value, policy)?,
            "--mount" => check_mount_flag(value, policy)?,
            "--publish" => check_publish(value)?,
            _ => {}
        }
    }

    let Some(image) = image else {
        return Err(refusal(&format!("'{sub}' needs an image.")));
    };
    check_image(image, policy)?;

    if !policy.name_prefixes.is_empty() {
        match name {
            None => {
                return Err(refusal(&format!(
                    "'{sub}' needs --name with a name starting with {}; a generated name would \
                     be outside this agent's scope.",
                    policy
                        .name_prefixes
                        .iter()
                        .map(|p| format!("'{p}'"))
                        .collect::<Vec<_>>()
                        .join(" or ")
                )));
            }
            Some(n) => check_name("container", n, policy, protected, true)?,
        }
    } else if let Some(n) = name {
        check_name("container", n, policy, protected, true)?;
    }
    Ok(())
}

/// `exec [opts] CONTAINER COMMAND...`: the options and the container are
/// checked; the command runs inside the container.
fn check_exec(
    rest: &[String],
    policy: &ContainerPolicy,
    protected: &[String],
) -> Result<(), String> {
    const VALUE: &[&str] = &[
        "-e",
        "--env",
        "--env-file",
        "-w",
        "--workdir",
        "-u",
        "--user",
        "--detach-keys",
        "--preserve-fds",
    ];
    const BOOL: &[&str] = &["--interactive", "--tty", "--detach", "--help"];
    let mut i = 0;
    while i < rest.len() {
        let arg = rest[i].as_str();
        if !arg.starts_with('-') || arg.len() == 1 {
            return check_name("container", arg, policy, protected, true);
        }
        let (flag, inline) = split_flag(arg);
        match flag {
            "--privileged" => {
                return Err(refusal("exec --privileged gives the process the whole host."));
            }
            "--latest" | "-l" => {
                return Err(refusal("--latest picks a container by age; name it instead."));
            }
            _ => {}
        }
        if VALUE.contains(&flag) {
            i += if inline.is_none() { 2 } else { 1 };
            continue;
        }
        if BOOL.contains(&flag) {
            i += 1;
            continue;
        }
        // -i, -t, -d, -it, -dit ...
        if !flag.starts_with("--") && flag[1..].chars().all(|c| matches!(c, 'i' | 't' | 'd')) {
            i += 1;
            continue;
        }
        return Err(refusal(&format!(
            "'{flag}' is not an option the kernel knows for 'exec'; it is refused."
        )));
    }
    Err(refusal("'exec' needs the container's name."))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn policy() -> ContainerPolicy {
        ContainerPolicy {
            name_prefixes: vec!["freeco-".to_string()],
            images: vec!["ghcr.io/freecodao/*".to_string(), "localhost/*".to_string()],
            allow_host_network: false,
        }
    }

    fn protected() -> Vec<String> {
        vec!["freeco-daemon".to_string()]
    }

    fn ok(cmd: &str) {
        if let Err(e) = check_command(cmd, &policy(), &protected()) {
            panic!("expected '{cmd}' to be allowed, got: {e}");
        }
    }

    fn refused(cmd: &str) {
        assert!(
            check_command(cmd, &policy(), &protected()).is_err(),
            "expected '{cmd}' to be refused"
        );
    }

    #[test]
    fn non_container_commands_pass() {
        ok("ls -la");
        ok("echo hello");
        ok("curl -s https://example.com");
    }

    #[test]
    fn read_only_listing_passes() {
        ok("podman ps -a");
        ok("podman ps -a --format json");
        ok("podman images");
        ok("podman version");
        ok("podman --version");
        ok("podman info");
        ok("podman container ls -a");
        ok("podman image ls");
        ok("podman volume ls");
        ok("podman network ls");
        ok("podman system df");
    }

    #[test]
    fn inspect_and_logs_are_scoped() {
        ok("podman inspect freeco-crm");
        ok("podman inspect --format '{{.State.Status}}' freeco-crm");
        ok("podman logs --tail 100 freeco-omniroute");
        ok("podman logs -f --since 10m freeco-crm");
        ok("podman port freeco-crm");
        ok("podman top freeco-crm pid user");
        ok("podman inspect ghcr.io/freecodao/omniroute:latest");
        refused("podman inspect postgres");
        refused("podman logs other-service");
        // A value flag's value is not taken for a container name.
        ok("podman logs --tail 50 freeco-crm");
    }

    #[test]
    fn protected_container_is_readable_but_not_touchable() {
        ok("podman logs freeco-daemon");
        refused("podman stop freeco-daemon");
        refused("podman restart freeco-daemon");
        refused("podman rm -f freeco-daemon");
        refused("podman exec -it freeco-daemon sh");
        refused("podman commit freeco-daemon localhost/snap");
        refused("podman run -d --name freeco-daemon --replace ghcr.io/freecodao/x");
    }

    #[test]
    fn lifecycle_in_scope_passes() {
        ok("podman start freeco-crm");
        ok("podman stop -t 20 freeco-crm");
        ok("podman restart freeco-crm freeco-crm-worker");
        ok("podman rm -f freeco-crm-old");
        ok("podman kill -s TERM freeco-crm");
        ok("podman rename freeco-crm freeco-crm-old");
        ok("podman container stop freeco-crm");
        ok("podman healthcheck run freeco-crm");
    }

    #[test]
    fn lifecycle_out_of_scope_is_refused() {
        refused("podman stop postgres");
        refused("podman rm -f nginx");
        refused("podman restart freeco-crm postgres");
        refused("podman rename freeco-crm postgres");
        refused("podman stop -a");
        refused("podman stop --all");
        refused("podman rm --latest");
        refused("podman stop --filter name=x");
        refused("podman stop");
        refused("podman container rm other");
    }

    #[test]
    fn run_in_scope_passes() {
        ok("podman run -d --name freeco-app -p 127.0.0.1:8080:80 ghcr.io/freecodao/app:1.2");
        ok("podman run -d --name=freeco-app localhost/app");
        ok("podman create --name freeco-app -v freeco-app-data:/data localhost/app");
        ok("podman run --rm -it --name freeco-tmp localhost/app sh -c 'ls /'");
        ok("podman run -d --name freeco-app --mount type=volume,source=freeco-data,target=/d localhost/app");
        ok("podman run -d --name freeco-app --tmpfs /tmp --mount type=tmpfs,target=/run localhost/app");
        ok("podman run -d --name freeco-app -e TZ=UTC --env-file /home/u/.env localhost/app");
        ok("podman run -d --name freeco-app --network freeco-net localhost/app");
        ok("podman run -d --name freeco-app --network container:freeco-db localhost/app");
        ok("podman run -d --name freeco-app -p [::1]:8080:80 localhost/app");
        ok("podman run -d --name freeco-app -p127.0.0.1:8080:80 localhost/app");
        ok("podman run -d --name freeco-app --security-opt no-new-privileges localhost/app");
        ok("podman run -d --name freeco-app -v /data localhost/app");
        ok("podman container run -d --name freeco-app localhost/app");
        // Arguments after the image belong to the container's own command.
        ok("podman run --name freeco-app localhost/app ls -v --privileged");
    }

    #[test]
    fn run_names_are_scoped() {
        refused("podman run -d localhost/app");
        refused("podman run -d --name web localhost/app");
        refused("podman create --name postgres localhost/app");
    }

    #[test]
    fn run_images_are_allowlisted() {
        refused("podman run -d --name freeco-x docker.io/diegosouzapw/omniroute:latest");
        refused("podman run -d --name freeco-x alpine");
        refused("podman run -d --name freeco-x ghcr.io/someoneelse/app");
        refused("podman run -d --name freeco-x");
        ok("podman run -d --name freeco-x GHCR.IO/FreecoDAO/app");
    }

    #[test]
    fn host_escapes_are_refused() {
        refused("podman run --privileged --name freeco-x localhost/app");
        refused("podman run --name freeco-x --cap-add SYS_ADMIN localhost/app");
        refused("podman run --name freeco-x --cap-add=NET_ADMIN localhost/app");
        refused("podman run --name freeco-x --pid host localhost/app");
        refused("podman run --name freeco-x --pid=host localhost/app");
        refused("podman run --name freeco-x --ipc host localhost/app");
        refused("podman run --name freeco-x --uts host localhost/app");
        refused("podman run --name freeco-x --userns host localhost/app");
        refused("podman run --name freeco-x --network host localhost/app");
        refused("podman run --name freeco-x --net=host localhost/app");
        refused("podman run --name freeco-x --network ns:/proc/1/ns/net localhost/app");
        refused("podman run --name freeco-x --network container:postgres localhost/app");
        refused("podman run --name freeco-x --device /dev/sda localhost/app");
        refused("podman run --name freeco-x --security-opt seccomp=unconfined localhost/app");
        refused("podman run --name freeco-x --security-opt label=disable localhost/app");
        refused("podman run --name freeco-x --volumes-from freeco-db localhost/app");
        refused("podman run --name freeco-x --rootfs /srv/root");
        refused("podman run --name freeco-x --group-add keep-groups localhost/app");
    }

    #[test]
    fn host_mounts_are_refused() {
        refused("podman run --name freeco-x -v /:/host localhost/app");
        refused("podman run --name freeco-x -v /etc:/etc:ro localhost/app");
        refused("podman run --name freeco-x -v ./data:/data localhost/app");
        refused("podman run --name freeco-x -v ~/.ssh:/root/.ssh localhost/app");
        refused("podman run --name freeco-x --volume=/var/run/podman.sock:/s localhost/app");
        refused("podman run --name freeco-x -v/srv:/srv localhost/app");
        refused("podman run --name freeco-x --mount type=bind,source=/,target=/host localhost/app");
        refused("podman run --name freeco-x --mount source=/,target=/host localhost/app");
        refused("podman run --name freeco-x --mount type=volume,source=pgdata,target=/d localhost/app");
        refused("podman run --name freeco-x -v pgdata:/var/lib/postgresql localhost/app");
        refused("podman run --name freeco-x -v 'C:\\Users:/u' localhost/app");
    }

    #[test]
    fn publishing_must_be_loopback() {
        refused("podman run --name freeco-x -p 8080:80 localhost/app");
        refused("podman run --name freeco-x -p 80 localhost/app");
        refused("podman run --name freeco-x -p 0.0.0.0:8080:80 localhost/app");
        refused("podman run --name freeco-x --publish=:8080:80 localhost/app");
        refused("podman run --name freeco-x -P localhost/app");
        refused("podman run --name freeco-x -dP localhost/app");
        refused("podman run --name freeco-x --publish-all localhost/app");
        ok("podman run --name freeco-x -p localhost:8080:80 localhost/app");
        ok("podman run --name freeco-x --publish 127.0.0.1:8080:80/tcp localhost/app");
    }

    #[test]
    fn host_network_can_be_allowed() {
        let mut p = policy();
        p.allow_host_network = true;
        assert!(
            check_command("podman run --name freeco-x --network host localhost/app", &p, &[])
                .is_ok()
        );
        // Only the network; other host namespaces stay refused.
        assert!(
            check_command("podman run --name freeco-x --pid host localhost/app", &p, &[]).is_err()
        );
    }

    #[test]
    fn unknown_options_and_subcommands_are_refused() {
        refused("podman run --name freeco-x --mystery-flag v localhost/app");
        refused("podman run --name freeco-x -Z localhost/app");
        refused("podman build -t localhost/app .");
        refused("podman compose up");
        refused("podman-compose up -d");
        refused("docker-compose up -d");
        refused("podman kube play pod.yaml");
        refused("podman play kube pod.yaml");
        refused("podman cp freeco-x:/etc/passwd .");
        refused("podman system prune -af");
        refused("podman container prune");
        refused("podman image prune -a");
        refused("podman volume prune");
        refused("podman network create freeco-net");
        refused("podman --remote ps");
        refused("podman --root /tmp/x ps");
        refused("podman unshare cat /etc/shadow");
        refused("podman exec --mystery freeco-x ls");
    }

    #[test]
    fn exec_is_scoped() {
        ok("podman exec freeco-crm ls /app");
        ok("podman exec -it freeco-crm sh");
        ok("podman exec -e FOO=1 -w /app freeco-crm env");
        ok("podman exec --user 1000 freeco-crm id");
        refused("podman exec postgres psql");
        refused("podman exec --privileged freeco-crm sh");
        refused("podman exec -l ls");
        refused("podman exec");
    }

    #[test]
    fn pull_push_tag_rmi_commit_check_images() {
        ok("podman pull ghcr.io/freecodao/omniroute:v3.8.50");
        ok("podman pull --platform linux/amd64 ghcr.io/freecodao/omniroute");
        ok("podman image pull localhost/app");
        refused("podman pull docker.io/diegosouzapw/omniroute:latest");
        refused("podman pull alpine");
        refused("podman pull");
        ok("podman tag localhost/app:1 localhost/app:rollback");
        refused("podman tag localhost/app docker.io/me/app");
        ok("podman rmi localhost/app:old");
        refused("podman rmi docker.io/library/nginx");
        refused("podman rmi -a");
        ok("podman commit freeco-crm localhost/freeco-crm:backup");
        ok("podman commit -m 'before update' freeco-crm localhost/freeco-crm:backup");
        refused("podman commit postgres localhost/x");
        refused("podman commit freeco-crm docker.io/me/x");
        ok("podman push localhost/app ghcr.io/freecodao/app:1");
        refused("podman push localhost/app docker.io/me/app");
    }

    #[test]
    fn volumes_are_scoped() {
        ok("podman volume create freeco-app-data");
        ok("podman volume create --driver local freeco-app-data");
        ok("podman volume rm freeco-app-data");
        ok("podman volume inspect pgdata");
        refused("podman volume create pgdata");
        refused("podman volume create");
        refused("podman volume rm pgdata");
        refused("podman volume rm --all");
        refused("podman volume create -o device=/srv -o o=bind freeco-x");
        refused("podman volume create --driver nfs freeco-x");
    }

    #[test]
    fn wrappers_and_nesting_are_seen() {
        refused("/usr/bin/podman run --privileged --name freeco-x localhost/app");
        refused("sudo podman run --privileged --name freeco-x localhost/app");
        refused("env podman stop postgres");
        refused("sh -c 'podman run --privileged --name freeco-x localhost/app'");
        refused("docker run --privileged --name freeco-x localhost/app");
        refused("PODMAN.EXE stop postgres");
        ok("sh -c 'podman ps -a'");
    }

    #[test]
    fn variables_are_refused() {
        refused("podman run --name freeco-x $IMAGE");
        refused("podman stop $NAME");
    }

    #[test]
    fn argv_form_is_checked_too() {
        let argv: Vec<String> = ["podman", "run", "--privileged", "--name", "freeco-x", "localhost/a"]
            .iter()
            .map(|s| s.to_string())
            .collect();
        assert!(check_argv(&argv, &policy(), &protected()).is_err());
        let argv: Vec<String> = ["podman", "ps", "-a"].iter().map(|s| s.to_string()).collect();
        assert!(check_argv(&argv, &policy(), &protected()).is_ok());
    }

    #[test]
    fn empty_lists_leave_that_dimension_open() {
        let open = ContainerPolicy::default();
        assert!(check_command("podman run -d alpine", &open, &[]).is_ok());
        assert!(check_command("podman stop postgres", &open, &[]).is_ok());
        // The host-escape checks still apply.
        assert!(check_command("podman run --privileged alpine", &open, &[]).is_err());
        assert!(check_command("podman run -v /:/h alpine", &open, &[]).is_err());
        assert!(check_command("podman run -p 80:80 alpine", &open, &[]).is_err());
    }

    #[test]
    fn wildcard_matching() {
        assert!(wildcard_match("ghcr.io/freecodao/*", "ghcr.io/freecodao/a:1"));
        assert!(wildcard_match("localhost/*", "localhost/x/y@sha256:abc"));
        assert!(wildcard_match("*", "anything"));
        assert!(wildcard_match("a*c", "abbbc"));
        assert!(!wildcard_match("ghcr.io/freecodao/*", "ghcr.io/freecodaox/a"));
        assert!(!wildcard_match("ghcr.io/freecodao/*", "evil/ghcr.io/freecodao/a"));
        assert!(!wildcard_match("alpine", "alpine2"));
    }
}
