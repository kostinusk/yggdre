use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{BTreeMap, HashSet},
    env, fs,
    fs::{File, OpenOptions},
    io::Write,
    net::{IpAddr, TcpStream, ToSocketAddrs},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    process::{Command, Stdio},
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use url::Url;

const PUBLIC_PEERS_URL: &str = "https://publicpeers.neilalexander.dev/publicnodes.json";
const ADMIN_SPLIT: &str = "__YGGDRE_SPLIT__";
const MAX_PUBLIC_PEERS_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize)]
struct BinaryInfo {
    path: Option<String>,
    version: Option<String>,
}

#[derive(Serialize)]
struct ConfigCandidate {
    path: String,
    exists: bool,
    readable: bool,
    writable: bool,
    #[serde(rename = "parentWritable")]
    parent_writable: bool,
}

#[derive(Serialize)]
struct LaunchdStatus {
    label: String,
    found: bool,
    state: Option<String>,
    pid: Option<u32>,
    path: Option<String>,
}

#[derive(Serialize)]
struct Environment {
    os: String,
    arch: String,
    yggdrasil: BinaryInfo,
    yggdrasilctl: BinaryInfo,
    #[serde(rename = "configPath")]
    config_path: Option<String>,
    #[serde(rename = "configCandidates")]
    config_candidates: Vec<ConfigCandidate>,
    #[serde(rename = "processRunning")]
    process_running: bool,
    launchd: LaunchdStatus,
    notes: Vec<String>,
}

#[derive(Serialize)]
struct NodeStatus {
    #[serde(rename = "processRunning")]
    process_running: bool,
    #[serde(rename = "adminApiOk")]
    admin_api_ok: bool,
    error: Option<String>,
    #[serde(rename = "buildName")]
    build_name: Option<String>,
    #[serde(rename = "buildVersion")]
    build_version: Option<String>,
    #[serde(rename = "ipv6Address")]
    ipv6_address: Option<String>,
    subnet: Option<String>,
    #[serde(rename = "publicKey")]
    public_key: Option<String>,
    coords: Option<String>,
    #[serde(rename = "peerCount")]
    peer_count: usize,
    #[serde(rename = "selfInfo")]
    self_info: Option<Value>,
}

#[derive(Debug, Deserialize)]
struct RawPublicPeer {
    up: Option<bool>,
    key: Option<String>,
    response_ms: Option<u64>,
    last_seen: Option<u64>,
    proto_minor: Option<u64>,
    priority: Option<i64>,
}

#[derive(Serialize)]
struct PublicPeer {
    uri: String,
    source: String,
    region: String,
    scheme: String,
    up: bool,
    key: Option<String>,
    #[serde(rename = "responseMs")]
    response_ms: Option<u64>,
    #[serde(rename = "lastSeen")]
    last_seen: Option<u64>,
    #[serde(rename = "protoMinor")]
    proto_minor: Option<u64>,
    priority: Option<i64>,
}

#[derive(Serialize)]
struct ProbeResult {
    uri: String,
    ok: bool,
    #[serde(rename = "localMs")]
    local_ms: Option<u128>,
    message: String,
}

#[derive(Serialize)]
struct ApplyResult {
    #[serde(rename = "backupPath")]
    backup_path: Option<String>,
    message: String,
}

struct Captured {
    success: bool,
    stdout: String,
    stderr: String,
}

#[tauri::command]
fn detect_environment() -> Environment {
    let yggdrasil = binary_info("yggdrasil");
    let yggdrasilctl = binary_info("yggdrasilctl");
    let config_candidates = config_candidates();
    let config_path = config_candidates
        .iter()
        .find(|candidate| candidate.exists)
        .or_else(|| config_candidates.first())
        .map(|candidate| candidate.path.clone());
    let process_running = process_running();
    let launchd = launchd_status();
    let mut notes = Vec::new();

    if yggdrasil.path.is_none() {
        notes.push("yggdrasil binary not found in PATH or Homebrew locations.".to_string());
    }
    if yggdrasilctl.path.is_none() {
        notes.push("yggdrasilctl binary not found; node inspection will be limited.".to_string());
    }
    if config_candidates
        .iter()
        .any(|candidate| candidate.exists && !candidate.readable)
    {
        notes.push("Config exists but is protected by macOS permissions. Enable admin prompt for apply/read actions.".to_string());
    }
    if process_running && !launchd.found {
        notes.push(
            "Yggdrasil process is running, but launchd label system/yggdrasil was not found."
                .to_string(),
        );
    }

    Environment {
        os: env::consts::OS.to_string(),
        arch: env::consts::ARCH.to_string(),
        yggdrasil,
        yggdrasilctl,
        config_path,
        config_candidates,
        process_running,
        launchd,
        notes,
    }
}

#[tauri::command]
fn get_node_status(use_admin: bool) -> Result<NodeStatus, String> {
    let process_running = process_running();
    let ctl = if use_admin {
        find_fixed_binary("yggdrasilctl")
    } else {
        find_binary("yggdrasilctl")
    };
    let Some(ctl) = ctl else {
        return Ok(NodeStatus {
            process_running,
            admin_api_ok: false,
            error: Some("yggdrasilctl not found".to_string()),
            build_name: None,
            build_version: None,
            ipv6_address: None,
            subnet: None,
            public_key: None,
            coords: None,
            peer_count: 0,
            self_info: None,
        });
    };

    let pair = if use_admin {
        run_yggdrasilctl_pair_admin(&ctl)
    } else {
        run_yggdrasilctl_pair(&ctl)
    };

    let (self_json, peers_json) = match pair {
        Ok(pair) => pair,
        Err(error) => {
            return Ok(NodeStatus {
                process_running,
                admin_api_ok: false,
                error: Some(error),
                build_name: None,
                build_version: None,
                ipv6_address: None,
                subnet: None,
                public_key: None,
                coords: None,
                peer_count: 0,
                self_info: None,
            });
        }
    };

    let self_value: Value = serde_json::from_str(&self_json)
        .map_err(|error| format!("Could not parse getSelf JSON: {error}"))?;
    let peers_value: Value = serde_json::from_str(&peers_json)
        .map_err(|error| format!("Could not parse getPeers JSON: {error}"))?;
    let self_response = response_value(&self_value).cloned();

    Ok(NodeStatus {
        process_running,
        admin_api_ok: true,
        error: None,
        build_name: find_string_by_keys(&self_value, &["build_name", "buildName", "Build name"]),
        build_version: find_string_by_keys(
            &self_value,
            &["build_version", "buildVersion", "Build version"],
        ),
        ipv6_address: find_string_by_keys(
            &self_value,
            &["ipv6_addr", "ipv6_address", "address", "IPv6 address"],
        ),
        subnet: find_string_by_keys(&self_value, &["subnet", "IPv6 subnet"]),
        public_key: find_string_by_keys(
            &self_value,
            &[
                "key",
                "public_key",
                "publicKey",
                "box_pub_key",
                "encryption_public_key",
            ],
        ),
        coords: find_display_by_keys(&self_value, &["coords", "coordinates"]),
        peer_count: count_response_records(&peers_value),
        self_info: self_response,
    })
}

#[tauri::command]
fn fetch_public_peers() -> Result<Vec<PublicPeer>, String> {
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()
        .map_err(|error| format!("Could not build HTTP client: {error}"))?;
    let response = client
        .get(PUBLIC_PEERS_URL)
        .send()
        .map_err(|error| format!("Could not fetch public peers: {error}"))?;
    if !response.status().is_success() {
        return Err(format!(
            "Public peers endpoint returned HTTP {}",
            response.status()
        ));
    }

    let body = response
        .bytes()
        .map_err(|error| format!("Could not read public peers response: {error}"))?;
    if body.len() > MAX_PUBLIC_PEERS_BYTES {
        return Err(format!(
            "Public peers response is too large: {} bytes",
            body.len()
        ));
    }

    let raw: BTreeMap<String, BTreeMap<String, RawPublicPeer>> = serde_json::from_slice(&body)
        .map_err(|error| format!("Could not parse public peers JSON: {error}"))?;
    let mut peers = Vec::new();

    for (source, entries) in raw {
        let region = source.trim_end_matches(".md").replace(['_', '-'], " ");
        for (uri, peer) in entries {
            peers.push(PublicPeer {
                scheme: uri
                    .split_once("://")
                    .map(|(scheme, _)| scheme)
                    .unwrap_or("unknown")
                    .to_string(),
                uri,
                source: source.clone(),
                region: region.clone(),
                up: peer.up.unwrap_or(false),
                key: peer.key,
                response_ms: peer.response_ms,
                last_seen: peer.last_seen,
                proto_minor: peer.proto_minor,
                priority: peer.priority,
            });
        }
    }

    peers.sort_by(|left, right| {
        right
            .up
            .cmp(&left.up)
            .then_with(|| {
                left.response_ms
                    .unwrap_or(u64::MAX)
                    .cmp(&right.response_ms.unwrap_or(u64::MAX))
            })
            .then_with(|| left.uri.cmp(&right.uri))
    });

    Ok(peers)
}

#[tauri::command]
fn probe_peers(uris: Vec<String>) -> Vec<ProbeResult> {
    let handles = uris
        .into_iter()
        .take(24)
        .map(|uri| {
            let fallback_uri = uri.clone();
            (fallback_uri, std::thread::spawn(move || probe_peer(uri)))
        })
        .collect::<Vec<_>>();

    handles
        .into_iter()
        .map(|(fallback_uri, handle)| {
            handle.join().unwrap_or(ProbeResult {
                uri: fallback_uri,
                ok: false,
                local_ms: None,
                message: "Probe worker failed.".to_string(),
            })
        })
        .collect()
}

#[tauri::command]
fn apply_peers_to_config(
    config_path: String,
    peers: Vec<String>,
    use_admin: bool,
    restart: bool,
) -> Result<ApplyResult, String> {
    if peers.is_empty() {
        return Err("Select at least one peer before applying config.".to_string());
    }
    validate_peer_uris(&peers)?;
    ensure_known_config_path(&config_path)?;

    if use_admin {
        let backup = apply_peers_admin(&config_path, &peers, restart)?;
        return Ok(ApplyResult {
            backup_path: Some(backup),
            message: if restart {
                "Peers written with admin privileges; daemon restart requested.".to_string()
            } else {
                "Peers written with admin privileges.".to_string()
            },
        });
    }

    let backup = write_peers_direct(&config_path, &peers)?;
    if restart {
        restart_direct()?;
    }

    Ok(ApplyResult {
        backup_path: Some(backup),
        message: if restart {
            "Peers written; daemon restart requested.".to_string()
        } else {
            "Peers written.".to_string()
        },
    })
}

#[tauri::command]
fn restart_yggdrasil(use_admin: bool) -> Result<ApplyResult, String> {
    if use_admin {
        run_privileged_shell("/bin/launchctl kickstart -k system/yggdrasil")?;
    } else {
        restart_direct()?;
    }

    Ok(ApplyResult {
        backup_path: None,
        message: "Yggdrasil restart requested through launchd.".to_string(),
    })
}

#[tauri::command]
fn install_yggdrasil_with_brew() -> Result<ApplyResult, String> {
    if let Some(path) = find_binary("yggdrasil") {
        return Ok(ApplyResult {
            backup_path: None,
            message: format!("Yggdrasil already installed at {}.", path_to_string(path)),
        });
    }

    let brew = find_brew().ok_or_else(|| {
        "Homebrew not found. Install Yggdrasil from https://github.com/yggdrasil-network/yggdrasil-go/releases/latest or install Homebrew first.".to_string()
    })?;
    let captured = run_path_command(&brew, &["install", "yggdrasil"])?;
    if !captured.success {
        return Err(join_output(captured));
    }

    Ok(ApplyResult {
        backup_path: None,
        message: "Yggdrasil installed with Homebrew.".to_string(),
    })
}

#[tauri::command]
fn create_default_config(config_path: String, use_admin: bool) -> Result<ApplyResult, String> {
    ensure_known_config_path_for_setup(&config_path)?;
    if Path::new(&config_path).exists() {
        return Ok(ApplyResult {
            backup_path: None,
            message: format!("Config already exists: {config_path}"),
        });
    }

    if use_admin {
        let yggdrasil = find_fixed_binary("yggdrasil")
            .ok_or_else(|| "yggdrasil binary not found".to_string())?;
        let script = format!(
            "CONFIG={config}; umask 077; {ygg} -genconf > \"$CONFIG\"; /usr/sbin/chown root:wheel \"$CONFIG\"; /bin/chmod 600 \"$CONFIG\"; /bin/echo \"Created config: $CONFIG\"",
            config = shell_quote(&config_path),
            ygg = shell_quote(&path_to_string(yggdrasil)),
        );
        let output = run_privileged_shell(&script)?;
        return Ok(ApplyResult {
            backup_path: None,
            message: output.trim().to_string(),
        });
    }

    let yggdrasil =
        find_binary("yggdrasil").ok_or_else(|| "yggdrasil binary not found".to_string())?;
    let captured = run_path_command(&yggdrasil, &["-genconf"])?;
    if !captured.success {
        return Err(join_output(captured));
    }
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&config_path)
        .map_err(|error| format!("Could not create config `{config_path}`: {error}"))?;
    file.write_all(captured.stdout.as_bytes())
        .map_err(|error| format!("Could not write config `{config_path}`: {error}"))?;

    Ok(ApplyResult {
        backup_path: None,
        message: format!("Created config: {config_path}"),
    })
}

#[tauri::command]
fn install_launchd_service(config_path: String, use_admin: bool) -> Result<ApplyResult, String> {
    ensure_known_config_path(&config_path)?;
    let yggdrasil = find_fixed_binary("yggdrasil").ok_or_else(|| {
        "yggdrasil binary not found in fixed system/Homebrew locations".to_string()
    })?;
    let plist = launchd_plist(&path_to_string(yggdrasil), &config_path);
    let target = "/Library/LaunchDaemons/yggdrasil.plist";

    if !use_admin {
        return Err("Installing a LaunchDaemon requires macOS admin privileges.".to_string());
    }

    let script = format!(
        "TARGET={target}; PLIST={plist}; /bin/echo \"$PLIST\" > \"$TARGET\"; /usr/sbin/chown root:wheel \"$TARGET\"; /bin/chmod 644 \"$TARGET\"; /bin/launchctl bootout system \"$TARGET\" >/dev/null 2>&1 || true; /bin/launchctl bootstrap system \"$TARGET\"; /bin/launchctl enable system/yggdrasil; /bin/launchctl kickstart -k system/yggdrasil; /bin/echo \"Installed launchd daemon: $TARGET\"",
        target = shell_quote(target),
        plist = shell_quote(&plist),
    );
    let output = run_privileged_shell(&script)?;
    Ok(ApplyResult {
        backup_path: None,
        message: output.trim().to_string(),
    })
}

#[tauri::command]
fn open_config_location(path: String) -> Result<ApplyResult, String> {
    let target = Path::new(&path);
    if !target.exists() {
        return Err(format!("Config path does not exist: {path}"));
    }
    let captured = run_command("/usr/bin/open", &["-R", &path])?;
    if !captured.success {
        return Err(join_output(captured));
    }
    Ok(ApplyResult {
        backup_path: None,
        message: format!("Revealed {path} in Finder."),
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![
            detect_environment,
            get_node_status,
            fetch_public_peers,
            probe_peers,
            apply_peers_to_config,
            restart_yggdrasil,
            install_yggdrasil_with_brew,
            create_default_config,
            install_launchd_service,
            open_config_location,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Yggdre");
}

fn binary_info(name: &str) -> BinaryInfo {
    let path = find_binary(name);
    let version = path.as_ref().and_then(|path| binary_version(path, name));
    BinaryInfo {
        path: path.map(path_to_string),
        version,
    }
}

fn find_binary(name: &str) -> Option<PathBuf> {
    let mut seen = HashSet::new();
    let mut dirs: Vec<PathBuf> = env::var_os("PATH")
        .map(|paths| env::split_paths(&paths).collect())
        .unwrap_or_default();
    dirs.extend([
        PathBuf::from("/usr/local/bin"),
        PathBuf::from("/opt/homebrew/bin"),
        PathBuf::from("/usr/bin"),
        PathBuf::from("/bin"),
    ]);

    for dir in dirs {
        let candidate = dir.join(name);
        let key = path_to_string(candidate.clone());
        if seen.insert(key) && candidate.is_file() {
            return Some(candidate);
        }
    }

    None
}

fn find_fixed_binary(name: &str) -> Option<PathBuf> {
    ["/usr/local/bin", "/opt/homebrew/bin", "/usr/bin", "/bin"]
        .into_iter()
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| candidate.is_file())
}

fn find_brew() -> Option<PathBuf> {
    ["/opt/homebrew/bin/brew", "/usr/local/bin/brew"]
        .into_iter()
        .map(PathBuf::from)
        .find(|candidate| candidate.is_file())
}

fn binary_version(path: &Path, name: &str) -> Option<String> {
    let primary = if name == "yggdrasilctl" {
        "-version"
    } else {
        "--version"
    };
    let fallback = if primary == "--version" {
        "-version"
    } else {
        "--version"
    };
    command_version(path, primary).or_else(|| command_version(path, fallback))
}

fn command_version(path: &Path, flag: &str) -> Option<String> {
    let output = Command::new(path).arg(flag).output().ok()?;
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let trimmed = combined.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

fn config_candidates() -> Vec<ConfigCandidate> {
    [
        "/etc/yggdrasil.conf",
        "/etc/yggdrasil/yggdrasil.conf",
        "/usr/local/etc/yggdrasil.conf",
        "/opt/homebrew/etc/yggdrasil.conf",
    ]
    .into_iter()
    .map(|path| {
        let path_ref = Path::new(path);
        ConfigCandidate {
            path: path.to_string(),
            exists: path_ref.exists(),
            readable: File::open(path_ref).is_ok(),
            writable: OpenOptions::new().write(true).open(path_ref).is_ok(),
            parent_writable: path_ref.parent().is_some_and(parent_writable),
        }
    })
    .collect()
}

fn parent_writable(parent: &Path) -> bool {
    let probe = parent.join(format!(
        ".yggdre-write-test-{}-{}",
        std::process::id(),
        unix_timestamp_nanos()
    ));
    match OpenOptions::new().write(true).create_new(true).open(&probe) {
        Ok(_) => {
            let _ = fs::remove_file(probe);
            true
        }
        Err(_) => false,
    }
}

fn launchd_status() -> LaunchdStatus {
    let captured = match run_command("/bin/launchctl", &["print", "system/yggdrasil"]) {
        Ok(captured) => captured,
        Err(error) => {
            return LaunchdStatus {
                label: "yggdrasil".to_string(),
                found: false,
                state: None,
                pid: None,
                path: Some(error),
            };
        }
    };

    if !captured.success {
        return LaunchdStatus {
            label: "yggdrasil".to_string(),
            found: false,
            state: None,
            pid: None,
            path: None,
        };
    }

    let state = parse_launchd_string(&captured.stdout, "state = ");
    let path = parse_launchd_string(&captured.stdout, "path = ");
    let pid =
        parse_launchd_string(&captured.stdout, "pid = ").and_then(|pid| pid.parse::<u32>().ok());

    LaunchdStatus {
        label: "yggdrasil".to_string(),
        found: true,
        state,
        pid,
        path,
    }
}

fn parse_launchd_string(source: &str, prefix: &str) -> Option<String> {
    source.lines().find_map(|line| {
        let trimmed = line.trim();
        trimmed
            .strip_prefix(prefix)
            .map(|value| value.trim().to_string())
    })
}

fn process_running() -> bool {
    Command::new("/usr/bin/pgrep")
        .args(["-x", "yggdrasil"])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn run_yggdrasilctl_pair(ctl: &Path) -> Result<(String, String), String> {
    let self_result = run_path_command(ctl, &["-json", "getSelf"])?;
    if !self_result.success {
        return Err(join_output(self_result));
    }

    let peers_result = run_path_command(ctl, &["-json", "getPeers"])?;
    if !peers_result.success {
        return Err(join_output(peers_result));
    }

    Ok((self_result.stdout, peers_result.stdout))
}

fn run_yggdrasilctl_pair_admin(ctl: &Path) -> Result<(String, String), String> {
    let ctl = shell_quote(&path_to_string(ctl.to_path_buf()));
    let script = format!("{ctl} -json getSelf\nprintf '\n{ADMIN_SPLIT}\n'\n{ctl} -json getPeers");
    let output = run_privileged_shell(&script)?.replace('\r', "\n");
    let Some((self_part, peers_part)) = output.split_once(ADMIN_SPLIT) else {
        return Err("Admin yggdrasilctl output did not contain expected separator.".to_string());
    };
    Ok((self_part.trim().to_string(), peers_part.trim().to_string()))
}

fn response_value(value: &Value) -> Option<&Value> {
    value.get("response").or(Some(value))
}

fn find_string_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key).and_then(value_to_string) {
                    return Some(found);
                }
            }
            map.values()
                .find_map(|child| find_string_by_keys(child, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_string_by_keys(child, keys)),
        _ => None,
    }
}

fn find_display_by_keys(value: &Value, keys: &[&str]) -> Option<String> {
    match value {
        Value::Object(map) => {
            for key in keys {
                if let Some(found) = map.get(*key) {
                    return Some(match found {
                        Value::String(text) => text.clone(),
                        _ => found.to_string(),
                    });
                }
            }
            map.values()
                .find_map(|child| find_display_by_keys(child, keys))
        }
        Value::Array(items) => items
            .iter()
            .find_map(|child| find_display_by_keys(child, keys)),
        _ => None,
    }
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        _ => None,
    }
}

fn count_response_records(value: &Value) -> usize {
    match response_value(value) {
        Some(Value::Array(items)) => items.len(),
        Some(Value::Object(map)) => {
            if let Some(Value::Array(items)) = map.get("peers") {
                items.len()
            } else {
                map.len()
            }
        }
        _ => 0,
    }
}

fn probe_peer(uri: String) -> ProbeResult {
    let parsed = match Url::parse(&uri) {
        Ok(parsed) => parsed,
        Err(error) => {
            return ProbeResult {
                uri,
                ok: false,
                local_ms: None,
                message: format!("Invalid URI: {error}"),
            };
        }
    };

    match parsed.scheme() {
        "tcp" | "tls" | "ws" | "wss" => probe_tcp_uri(uri, parsed),
        "quic" => ProbeResult {
            uri,
            ok: false,
            local_ms: None,
            message: "QUIC uses UDP; TCP probe skipped.".to_string(),
        },
        scheme => ProbeResult {
            uri,
            ok: false,
            local_ms: None,
            message: format!("Unsupported local probe scheme: {scheme}"),
        },
    }
}

fn probe_tcp_uri(uri: String, parsed: Url) -> ProbeResult {
    let Some(host) = parsed.host_str() else {
        return ProbeResult {
            uri,
            ok: false,
            local_ms: None,
            message: "Missing host.".to_string(),
        };
    };
    let Some(port) = parsed.port() else {
        return ProbeResult {
            uri,
            ok: false,
            local_ms: None,
            message: "Missing port.".to_string(),
        };
    };

    let addresses = match (host, port).to_socket_addrs() {
        Ok(addresses) => addresses.collect::<Vec<_>>(),
        Err(error) => {
            return ProbeResult {
                uri,
                ok: false,
                local_ms: None,
                message: format!("DNS failed: {error}"),
            };
        }
    };

    if addresses.is_empty() {
        return ProbeResult {
            uri,
            ok: false,
            local_ms: None,
            message: "DNS returned no addresses.".to_string(),
        };
    }

    let timeout = Duration::from_secs(2);
    let mut last_error = None;
    for address in addresses {
        let started = Instant::now();
        match TcpStream::connect_timeout(&address, timeout) {
            Ok(_) => {
                return ProbeResult {
                    uri,
                    ok: true,
                    local_ms: Some(started.elapsed().as_millis()),
                    message: "TCP reachable.".to_string(),
                };
            }
            Err(error) => last_error = Some(error.to_string()),
        }
    }

    ProbeResult {
        uri,
        ok: false,
        local_ms: None,
        message: last_error.unwrap_or_else(|| "Connection failed.".to_string()),
    }
}

fn validate_peer_uris(peers: &[String]) -> Result<(), String> {
    for peer in peers {
        let parsed =
            Url::parse(peer).map_err(|error| format!("Invalid peer URI `{peer}`: {error}"))?;
        match parsed.scheme() {
            "tcp" | "tls" | "quic" | "ws" | "wss" | "socks" => {}
            scheme => return Err(format!("Unsupported peer scheme `{scheme}` in `{peer}`")),
        }
        validate_peer_target(peer, &parsed)?;
    }
    Ok(())
}

fn validate_peer_target(peer: &str, parsed: &Url) -> Result<(), String> {
    let Some(host) = parsed.host_str() else {
        return Err(format!("Peer URI has no host: `{peer}`"));
    };

    if let Ok(address) = host.parse::<IpAddr>() {
        return validate_public_ip(peer, address);
    }

    let Some(port) = parsed.port() else {
        return Err(format!("Peer URI has no port: `{peer}`"));
    };

    let addresses = (host, port).to_socket_addrs().map_err(|error| {
        format!("Could not resolve peer `{peer}` before applying config: {error}")
    })?;
    for address in addresses {
        validate_public_ip(peer, address.ip())?;
    }

    Ok(())
}

fn validate_public_ip(peer: &str, address: IpAddr) -> Result<(), String> {
    let forbidden = match address {
        IpAddr::V4(address) => {
            address.is_private()
                || address.is_loopback()
                || address.is_link_local()
                || address.is_multicast()
                || address.is_unspecified()
                || address.is_broadcast()
        }
        IpAddr::V6(address) => {
            let first_segment = address.segments()[0];
            let unique_local = (first_segment & 0xfe00) == 0xfc00;
            let link_local = (first_segment & 0xffc0) == 0xfe80;
            unique_local
                || link_local
                || address.is_loopback()
                || address.is_multicast()
                || address.is_unspecified()
        }
    };

    if forbidden {
        Err(format!(
            "Refusing private, loopback, link-local, multicast, or unspecified peer target `{address}` from `{peer}`"
        ))
    } else {
        Ok(())
    }
}

fn ensure_known_config_path(config_path: &str) -> Result<(), String> {
    let requested = Path::new(config_path);
    if !requested.is_absolute() {
        return Err(format!("Config path must be absolute: `{config_path}`"));
    }
    reject_symlink(config_path)?;
    let allowed = config_candidates()
        .iter()
        .any(|candidate| same_path(requested, Path::new(&candidate.path)));

    if allowed {
        Ok(())
    } else {
        Err(format!(
            "Refusing to modify unknown config path `{config_path}`. Use a standard Yggdrasil config location."
        ))
    }
}

fn ensure_known_config_path_for_setup(config_path: &str) -> Result<(), String> {
    let requested = Path::new(config_path);
    if !requested.is_absolute() {
        return Err(format!("Config path must be absolute: `{config_path}`"));
    }
    if requested.exists() {
        return ensure_known_config_path(config_path);
    }
    let allowed = config_candidates()
        .iter()
        .any(|candidate| requested == Path::new(&candidate.path));
    if allowed {
        Ok(())
    } else {
        Err(format!(
            "Refusing to create unknown config path `{config_path}`. Use a standard Yggdrasil config location."
        ))
    }
}

fn same_path(left: &Path, right: &Path) -> bool {
    if left == right {
        return true;
    }

    match (fs::canonicalize(left), fs::canonicalize(right)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

fn reject_symlink(path: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("Could not inspect config path `{path}`: {error}"))?;
    if metadata.file_type().is_symlink() {
        Err(format!("Refusing to modify symlinked config path `{path}`"))
    } else {
        Ok(())
    }
}

fn write_staged_config(rendered: &str) -> Result<PathBuf, String> {
    let staged_dir = env::temp_dir().join(format!(
        "yggdre-config-{}-{}",
        std::process::id(),
        unix_timestamp_nanos()
    ));
    fs::create_dir(&staged_dir)
        .map_err(|error| format!("Could not create private staging dir: {error}"))?;
    fs::set_permissions(&staged_dir, fs::Permissions::from_mode(0o700))
        .map_err(|error| format!("Could not secure private staging dir: {error}"))?;
    let staged_path = staged_dir.join("yggdrasil.conf.json");
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&staged_path)
        .map_err(|error| format!("Could not create staged config: {error}"))?;
    file.write_all(rendered.as_bytes())
        .and_then(|_| file.write_all(b"\n"))
        .map_err(|error| format!("Could not write staged config: {error}"))?;
    Ok(staged_path)
}

fn launchd_plist(yggdrasil_path: &str, config_path: &str) -> String {
    let yggdrasil_path = xml_escape(yggdrasil_path);
    let config_path = xml_escape(config_path);
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?><!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\"><plist version=\"1.0\"><dict><key>Label</key><string>yggdrasil</string><key>ProgramArguments</key><array><string>{yggdrasil_path}</string><string>-useconffile</string><string>{config_path}</string></array><key>KeepAlive</key><true/><key>RunAtLoad</key><true/><key>ProcessType</key><string>Interactive</string><key>StandardOutPath</key><string>/tmp/yggdrasil.stdout.log</string><key>StandardErrorPath</key><string>/tmp/yggdrasil.stderr.log</string></dict></plist>"
    )
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

fn write_peers_direct(config_path: &str, peers: &[String]) -> Result<String, String> {
    reject_symlink(config_path)?;
    let yggdrasil =
        find_binary("yggdrasil").ok_or_else(|| "yggdrasil binary not found".to_string())?;
    let captured = run_path_command(
        &yggdrasil,
        &["-normaliseconf", "-json", "-useconffile", config_path],
    )?;
    if !captured.success {
        return Err(join_output(captured));
    }

    let mut config: Value = serde_json::from_str(&captured.stdout)
        .map_err(|error| format!("Could not parse normalised config JSON: {error}"))?;
    let Some(map) = config.as_object_mut() else {
        return Err("Normalised config is not a JSON object.".to_string());
    };
    map.insert(
        "Peers".to_string(),
        Value::Array(
            peers
                .iter()
                .map(|peer| Value::String(peer.clone()))
                .collect(),
        ),
    );

    let backup = backup_path(config_path);
    fs::copy(config_path, &backup)
        .map_err(|error| format!("Could not create backup `{backup}`: {error}"))?;
    let rendered = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("Could not render config JSON: {error}"))?;
    fs::write(config_path, format!("{rendered}\n"))
        .map_err(|error| format!("Could not write config `{config_path}`: {error}"))?;

    Ok(backup)
}

fn apply_peers_admin(config_path: &str, peers: &[String], restart: bool) -> Result<String, String> {
    reject_symlink(config_path)?;
    let yggdrasil =
        find_fixed_binary("yggdrasil").ok_or_else(|| "yggdrasil binary not found".to_string())?;
    let normalise_script = format!(
        "{} -normaliseconf -json -useconffile {}",
        shell_quote(&path_to_string(yggdrasil)),
        shell_quote(config_path)
    );
    let normalised = run_privileged_shell(&normalise_script)?;
    let mut config: Value = serde_json::from_str(&normalised)
        .map_err(|error| format!("Could not parse normalised config JSON: {error}"))?;
    let Some(map) = config.as_object_mut() else {
        return Err("Normalised config is not a JSON object.".to_string());
    };
    map.insert(
        "Peers".to_string(),
        Value::Array(
            peers
                .iter()
                .map(|peer| Value::String(peer.clone()))
                .collect(),
        ),
    );

    let rendered = serde_json::to_string_pretty(&config)
        .map_err(|error| format!("Could not render config JSON: {error}"))?;
    let staged_path = write_staged_config(&rendered)?;
    let mut install_script = format!(
        "CONFIG={config}; TMP={tmp}; BACKUP=\"${{CONFIG}}.yggdre-backup.$(/bin/date +%s)\"; MODE=$(/usr/bin/stat -f %Lp \"$CONFIG\"); OWNER=$(/usr/bin/stat -f %u \"$CONFIG\"); GROUP=$(/usr/bin/stat -f %g \"$CONFIG\"); /bin/cp -p \"$CONFIG\" \"$BACKUP\"; /usr/bin/install -m \"$MODE\" -o \"$OWNER\" -g \"$GROUP\" \"$TMP\" \"$CONFIG\"; /bin/rm -f \"$TMP\"; /bin/echo \"$BACKUP\"",
        config = shell_quote(config_path),
        tmp = shell_quote(&path_to_string(staged_path.clone())),
    );
    if restart {
        install_script.push_str("; /bin/launchctl kickstart -k system/yggdrasil");
    }

    let output = run_privileged_shell(&install_script);
    let _ = fs::remove_file(&staged_path);
    if let Some(parent) = staged_path.parent() {
        let _ = fs::remove_dir(parent);
    }
    let output = output?;
    output
        .lines()
        .find(|line| line.contains(".yggdre-backup."))
        .map(|line| line.trim().to_string())
        .ok_or_else(|| "Admin apply completed, but backup path was not reported.".to_string())
}

fn restart_direct() -> Result<(), String> {
    let captured = run_command("/bin/launchctl", &["kickstart", "-k", "system/yggdrasil"])?;
    if captured.success {
        Ok(())
    } else {
        Err(join_output(captured))
    }
}

fn run_command(program: &str, args: &[&str]) -> Result<Captured, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|error| format!("Could not run `{program}`: {error}"))?;
    Ok(Captured {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn run_path_command(program: &Path, args: &[&str]) -> Result<Captured, String> {
    let output = Command::new(program).args(args).output().map_err(|error| {
        format!(
            "Could not run `{}`: {error}",
            path_to_string(program.to_path_buf())
        )
    })?;
    Ok(Captured {
        success: output.status.success(),
        stdout: String::from_utf8_lossy(&output.stdout).to_string(),
        stderr: String::from_utf8_lossy(&output.stderr).to_string(),
    })
}

fn run_privileged_shell(script: &str) -> Result<String, String> {
    let shell_command = format!("/bin/sh -c {}", shell_quote(&format!("set -eu; {script}")));
    let apple_script = format!(
        "do shell script {} with administrator privileges",
        applescript_quote(&shell_command)
    );
    let captured = run_command("/usr/bin/osascript", &["-e", &apple_script])?;
    if captured.success {
        Ok(captured.stdout)
    } else {
        Err(join_output(captured))
    }
}

fn join_output(captured: Captured) -> String {
    let stdout = captured.stdout.trim();
    let stderr = captured.stderr.trim();
    match (stdout.is_empty(), stderr.is_empty()) {
        (true, true) => "Command failed without output.".to_string(),
        (false, true) => stdout.to_string(),
        (true, false) => stderr.to_string(),
        (false, false) => format!("{stdout}\n{stderr}"),
    }
}

fn backup_path(config_path: &str) -> String {
    format!("{config_path}.yggdre-backup.{}", unix_timestamp())
}

fn unix_timestamp() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn unix_timestamp_nanos() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn applescript_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn path_to_string(path: PathBuf) -> String {
    path.to_string_lossy().to_string()
}
