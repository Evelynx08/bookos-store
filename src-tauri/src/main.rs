#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::{Command, Stdio};
use std::io::Write;
use std::sync::Mutex;
use std::collections::HashMap;
use once_cell::sync::Lazy;
use base64::{Engine as _, engine::general_purpose};

#[derive(Clone, Default)]
struct OpState {
    pid: Option<u32>,
    pct: f64,
    downloaded: u64,
    total: u64,
}

/// Active operations: op_id → {pid, progress}. Read by `progress` cmd, set by
/// downloader thread, written by track/untrack.
static OPS: Lazy<Mutex<HashMap<String, OpState>>> = Lazy::new(|| Mutex::new(HashMap::new()));

fn track(op_id: &str, pid: u32) {
    if let Ok(mut g) = OPS.lock() {
        g.entry(op_id.to_string()).or_default().pid = Some(pid);
    }
}
fn untrack(op_id: &str) {
    if let Ok(mut g) = OPS.lock() { g.remove(op_id); }
}
fn set_progress(op_id: &str, pct: f64, downloaded: u64, total: u64) {
    if let Ok(mut g) = OPS.lock() {
        let e = g.entry(op_id.to_string()).or_default();
        e.pct = pct; e.downloaded = downloaded; e.total = total;
    }
}

#[tauri::command]
fn progress(op_id: String) -> serde_json::Value {
    if let Ok(g) = OPS.lock() {
        if let Some(s) = g.get(&op_id) {
            return serde_json::json!({
                "pct": s.pct, "downloaded": s.downloaded, "total": s.total, "active": true
            });
        }
    }
    serde_json::json!({ "pct": 100.0, "active": false })
}

/// Remote catalog endpoint. Single source of truth for available apps + their
/// latest versions + asset URLs. Served by the BookOS website (PHP backend),
/// not GitHub, so publishing a new app does not need a client recompile.
/// Override at runtime with env BOOKOS_CATALOG_URL.
fn catalog_url() -> String {
    std::env::var("BOOKOS_CATALOG_URL")
        .unwrap_or_else(|_| "https://bookos.es/api/store.json".to_string())
}

/// Fetch catalog from server with on-disk cache (TTL 10 min). On network
/// failure returns stale cache marked with `_stale: true`. Empty list if no
/// cache and no network.
fn fetch_catalog_cached() -> Vec<serde_json::Value> {
    let mut cache_path = match dirs::cache_dir() {
        Some(p) => p,
        None => return Vec::new(),
    };
    cache_path.push("bookos-store");
    let _ = std::fs::create_dir_all(&cache_path);
    cache_path.push("catalog.json");

    let fresh = std::fs::metadata(&cache_path).ok()
        .and_then(|m| m.modified().ok())
        .and_then(|t| t.elapsed().ok())
        .map(|d| d.as_secs() < 600)
        .unwrap_or(false);

    if fresh {
        if let Ok(s) = std::fs::read_to_string(&cache_path) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                return v.get("apps").and_then(|a| a.as_array()).cloned().unwrap_or_default();
            }
        }
    }

    let url = catalog_url();
    // Try primary URL; if it fails (e.g. server rewrite not configured), retry
    // with `.php` suffix directly — keeps client working even with a misconfigured Apache.
    let fallback_url = if url.ends_with(".json") {
        Some(format!("{}.php", url))
    } else { None };

    // Try each URL with strict TLS first; if TLS chain is broken (curl exit 60
    // = unable to verify cert), retry once with `-k` so the app keeps working
    // until the server cert is fixed. Set BOOKOS_INSECURE_TLS=1 to skip strict.
    let insecure_first = std::env::var("BOOKOS_INSECURE_TLS").ok().as_deref() == Some("1");
    for candidate in std::iter::once(url.as_str()).chain(fallback_url.as_deref()) {
        for insecure in if insecure_first { [true, false] } else { [false, true] } {
            let mut cmd = Command::new("curl");
            cmd.args(["-fsSL", "--max-time", "15",
                      "-H", "Accept: application/json",
                      "-H", "User-Agent: bookos-store"]);
            if insecure { cmd.arg("-k"); }
            cmd.arg(candidate);
            let out = cmd.output();
            if let Ok(o) = out {
                if o.status.success() {
                    let body = String::from_utf8_lossy(&o.stdout).to_string();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&body) {
                        let _ = std::fs::write(&cache_path, &body);
                        eprintln!("[bookos-store] catalog ok from {} (insecure={})", candidate, insecure);
                        return v.get("apps").and_then(|a| a.as_array()).cloned().unwrap_or_default();
                    }
                } else {
                    eprintln!("[bookos-store] curl {} exit={:?} stderr={}",
                        candidate, o.status.code(),
                        String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or(""));
                }
            }
        }
    }

    // Stale fallback.
    if let Ok(s) = std::fs::read_to_string(&cache_path) {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
            return v.get("apps").and_then(|a| a.as_array()).cloned().unwrap_or_default();
        }
    }
    Vec::new()
}

/// Package manager backend detected at runtime.
#[derive(Clone, Copy, PartialEq)]
enum Pm { Pacman, Dnf, Apt }

fn detect_pm() -> Pm {
    // Manual override via env (debug / testing).
    if let Ok(v) = std::env::var("BOOKOS_PM") {
        match v.to_lowercase().as_str() {
            "pacman" => return Pm::Pacman,
            "dnf"    => return Pm::Dnf,
            "apt"    => return Pm::Apt,
            _ => {}
        }
    }
    // Prefer /etc/os-release ID over scanning binaries: tools like dpkg/rpm can
    // be co-installed on Arch (e.g. for build-deps) and would mis-detect.
    if let Ok(s) = std::fs::read_to_string("/etc/os-release") {
        let id = s.lines().find_map(|l| l.strip_prefix("ID=")).unwrap_or("").trim_matches('"').to_lowercase();
        let id_like = s.lines().find_map(|l| l.strip_prefix("ID_LIKE=")).unwrap_or("").trim_matches('"').to_lowercase();
        let combined = format!("{} {}", id, id_like);
        if combined.split_whitespace().any(|w| matches!(w, "arch" | "cachyos" | "manjaro" | "endeavouros" | "garuda" | "artix")) {
            return Pm::Pacman;
        }
        if combined.split_whitespace().any(|w| matches!(w, "fedora" | "rhel" | "centos" | "rocky" | "almalinux" | "opensuse" | "suse")) {
            return Pm::Dnf;
        }
        if combined.split_whitespace().any(|w| matches!(w, "debian" | "ubuntu" | "linuxmint" | "pop" | "kali" | "raspbian")) {
            return Pm::Apt;
        }
    }
    // Fallback: scan binaries, but prefer pacman over dpkg.
    let exists = |p: &str| std::path::Path::new(p).exists();
    let has = |bin: &str| exists(&format!("/usr/bin/{}", bin)) || exists(&format!("/bin/{}", bin));
    if has("pacman") { Pm::Pacman }
    else if has("dnf") || has("rpm") { Pm::Dnf }
    else if has("dpkg") || has("apt") { Pm::Apt }
    else { Pm::Pacman }
}

/// Force-clear the local catalog cache so next list_apps does a fresh HTTP fetch.
/// Called by the Refresh button when admin publishes new versions.
#[tauri::command]
fn clear_catalog_cache() -> bool {
    if let Some(mut p) = dirs::cache_dir() {
        p.push("bookos-store");
        p.push("catalog.json");
        let _ = std::fs::remove_file(&p);
        return true;
    }
    false
}

#[tauri::command]
fn list_apps() -> serde_json::Value {
    let pm = detect_pm();
    let apps = fetch_catalog_cached();
    let enriched: Vec<serde_json::Value> = apps.into_iter().map(|mut a| {
        let pkg = a["pkg"].as_str().unwrap_or("").to_string();
        let installed = pkg_version(pm, &pkg);
        a["installed"] = serde_json::json!(installed);
        a
    }).collect();
    serde_json::json!(enriched)
}

fn pkg_version(pm: Pm, pkg: &str) -> Option<String> {
    match pm {
        Pm::Pacman => {
            let out = Command::new("pacman").args(["-Q", pkg]).output().ok()?;
            if !out.status.success() { return None; }
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            s.split_whitespace().nth(1).map(|v| v.to_string())
        }
        Pm::Dnf => {
            let out = Command::new("rpm").args(["-q", "--qf", "%{VERSION}-%{RELEASE}", pkg]).output().ok()?;
            if !out.status.success() { return None; }
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() || s.contains("not installed") { None } else { Some(s) }
        }
        Pm::Apt => {
            let out = Command::new("dpkg-query").args(["-W", "-f=${Version}", pkg]).output().ok()?;
            if !out.status.success() { return None; }
            let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
            if s.is_empty() { None } else { Some(s) }
        }
    }
}

#[tauri::command]
fn is_installed(pkg: String) -> bool {
    pkg_version(detect_pm(), &pkg).is_some()
}

/// Expose detected package manager + supported asset extensions to frontend.
#[tauri::command]
fn pm_info() -> serde_json::Value {
    match detect_pm() {
        Pm::Pacman => serde_json::json!({ "pm": "pacman", "exts": [".pkg.tar.zst"] }),
        Pm::Dnf => serde_json::json!({ "pm": "dnf", "exts": [".rpm"] }),
        Pm::Apt => serde_json::json!({ "pm": "apt", "exts": [".deb"] }),
    }
}

#[tauri::command]
fn launch_app(pkg: String) -> Result<(), String> {
    // Tauri-built apps install their binary at /usr/bin/<pkgname>
    Command::new(&pkg).spawn().map(|_| ())
        .map_err(|e| format!("No se pudo lanzar {}: {}", pkg, e))
}

/// Open the app's homepage (or BookOS site) in the browser as a fallback when
/// no compatible binary is available for this user's distro.
#[tauri::command]
fn open_release_page(repo: String) -> Result<(), String> {
    let pkg = repo.rsplit('/').next().unwrap_or(&repo).to_string();
    let url = fetch_catalog_cached().into_iter()
        .find(|a| a["pkg"].as_str() == Some(&pkg))
        .and_then(|a| a.get("html_url").and_then(|u| u.as_str()).map(String::from))
        .unwrap_or_else(|| "https://bookos.es/".to_string());
    Command::new("xdg-open").arg(&url).spawn().map(|_| ())
        .map_err(|e| e.to_string())
}

/// Run privileged command. If `password` provided, uses `sudo -S` and feeds it
/// via stdin. If empty, falls back to pkexec. If `op_id` set, registers PID for
/// cancellation.
fn run_priv(args: &[&str], password: &str, op_id: Option<&str>) -> Result<String, String> {
    // Build a clean PATH so spawned sudo/pkexec can find dnf/pacman/apt even
    // when Tauri launches us with a stripped env (e.g. via .desktop entry).
    let safe_path = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

    if password.is_empty() {
        let bin = if Command::new("which").arg("pkexec").output()
            .map(|o| o.status.success()).unwrap_or(false) { "pkexec" } else { "sudo" };
        eprintln!("[bookos-store] run_priv (no password) via {}: {:?}", bin, args);
        let child = Command::new(bin)
            .env("PATH", safe_path)
            .args(args)
            .stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| format!("Fallo al lanzar {}: {}", bin, e))?;
        if let Some(id) = op_id { track(id, child.id()); }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if let Some(id) = op_id { untrack(id); }
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr).to_string();
            let stdout = String::from_utf8_lossy(&out.stdout).to_string();
            let combined = if stderr.trim().is_empty() { stdout } else { stderr };
            eprintln!("[bookos-store] run_priv (no-pw) failed exit={:?}: {}", out.status.code(), combined);
            return Err(if combined.trim().is_empty() { format!("fallo sin output (exit {:?})", out.status.code()) } else { combined.trim().to_string() });
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    eprintln!("[bookos-store] run_priv (with sudo password): {:?}", args);
    let mut child = Command::new("sudo")
        .env("PATH", safe_path)
        .arg("-S").arg("-p").arg("")
        .args(args)
        .stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped())
        .spawn().map_err(|e| format!("Fallo al lanzar sudo: {}", e))?;
    if let Some(id) = op_id { track(id, child.id()); }
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(format!("{}\n", password).as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    if let Some(id) = op_id { untrack(id); }
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        if stderr.contains("incorrect password") || stderr.contains("Sorry, try again") {
            return Err("Contraseña incorrecta".into());
        }
        if stderr.contains("Terminated") || stderr.contains("signal: 15") {
            return Err("__cancelled__".into());
        }
        // Return full stderr+stdout so frontend dialog shows useful debug.
        let combined = if stderr.trim().is_empty() { stdout.clone() } else { stderr.clone() };
        eprintln!("[bookos-store] run_priv failed: exit={:?}\nstderr:\n{}\nstdout:\n{}",
            out.status.code(), stderr, stdout);
        return Err(if combined.trim().is_empty() {
            "Falló sin output".into()
        } else {
            combined.trim().to_string()
        });
    }
    Ok(stdout)
}

/// Cancel an in-flight op by sending SIGTERM to its tracked PID.
#[tauri::command]
fn cancel_op(op_id: String) -> bool {
    let pid = match OPS.lock().ok().and_then(|g| g.get(&op_id).and_then(|s| s.pid)) {
        Some(p) => p, None => return false,
    };
    Command::new("kill").arg("-TERM").arg(pid.to_string()).status()
        .map(|s| s.success()).unwrap_or(false)
}

/// Install a local package file. Backend picks pacman -U or dnf install based
/// on detected distro. If `password` is provided, uses sudo -S (custom dialog);
/// otherwise pkexec (system dialog).
#[tauri::command]
async fn install_pkg_file(path: String, password: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let canon = std::fs::canonicalize(&path)
        .map_err(|e| format!("Archivo no existe: {} ({})", path, e))?;
    let canon_str = canon.to_string_lossy().to_string();
    eprintln!("[bookos-store] install_pkg_file path={}", canon_str);
    let pm = detect_pm();
    eprintln!("[bookos-store] detected pm = {:?}", match pm { Pm::Pacman=>"pacman", Pm::Dnf=>"dnf", Pm::Apt=>"apt" });
    let args: Vec<&str> = match pm {
        Pm::Pacman => vec!["pacman", "-U", "--noconfirm", &canon_str],
        Pm::Dnf => vec!["dnf", "install", "-y", "--allowerasing", &canon_str],
        Pm::Apt => vec!["apt", "install", "-y", "--allow-downgrades", &canon_str],
    };
    eprintln!("[bookos-store] running: {:?}", args);
    run_priv(&args, password.as_deref().unwrap_or(""), op_id.as_deref())
}

/// Install a package by NAME from configured repos (no file download).
/// Use when the BookOS dnf repo is configured at /etc/yum.repos.d/bookos.repo.
/// Falls back gracefully on Apt/Pacman.
#[tauri::command]
async fn install_pkg_by_name(pkg: String, password: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-Sy", "--noconfirm", &pkg],
        Pm::Dnf => vec!["dnf", "install", "-y", "--refresh", &pkg],
        Pm::Apt => vec!["apt", "install", "-y", &pkg],
    };
    run_priv(&args, password.as_deref().unwrap_or(""), op_id.as_deref())
}

/// Upgrade all BookOS packages via system repos.
#[tauri::command]
async fn upgrade_all(password: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-Syu", "--noconfirm"],
        Pm::Dnf => vec!["dnf", "upgrade", "-y", "--refresh"],
        Pm::Apt => vec!["sh", "-c", "apt update && apt upgrade -y"],
    };
    run_priv(&args, password.as_deref().unwrap_or(""), op_id.as_deref())
}

/// Uninstall a package via pacman -Rs or dnf remove.
#[tauri::command]
async fn uninstall_pkg(pkg: String, password: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-Rs", "--noconfirm", &pkg],
        Pm::Dnf => vec!["dnf", "remove", "-y", &pkg],
        Pm::Apt => vec!["apt", "remove", "-y", &pkg],
    };
    run_priv(&args, password.as_deref().unwrap_or(""), op_id.as_deref())?;
    Ok(String::from("ok"))
}

/// Verify a sudo password without doing anything destructive.
#[tauri::command]
async fn verify_password(password: String) -> bool {
    if password.is_empty() { return false; }
    let mut child = match Command::new("sudo")
        .arg("-S").arg("-k").arg("-p").arg("").arg("true")
        .stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null())
        .spawn() { Ok(c) => c, Err(_) => return false };
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(format!("{}\n", password).as_bytes());
    }
    child.wait().map(|s| s.success()).unwrap_or(false)
}

/// Read app icon from hicolor theme and return as base64 PNG. Tries multiple
/// sizes and falls back to scalable SVG. Returns empty string if not found.
#[tauri::command]
fn get_icon(name: String) -> String {
    let sizes = ["256x256", "512x512", "128x128", "96x96", "64x64", "48x48"];
    for sz in sizes {
        let p = format!("/usr/share/icons/hicolor/{}/apps/{}.png", sz, name);
        if let Ok(bytes) = std::fs::read(&p) {
            return format!("data:image/png;base64,{}", general_purpose::STANDARD.encode(&bytes));
        }
    }
    let svg = format!("/usr/share/icons/hicolor/scalable/apps/{}.svg", name);
    if let Ok(bytes) = std::fs::read(&svg) {
        return format!("data:image/svg+xml;base64,{}", general_purpose::STANDARD.encode(&bytes));
    }
    String::new()
}

/// Download asset to ~/.cache/bookos-store/. Writes progress to OPS map for
/// frontend to poll via `progress(op_id)`. Cancellable via cancel_op(op_id).
#[tauri::command]
async fn download_pkg(url: String, filename: String, op_id: Option<String>) -> Result<String, String> {
    let mut dest = dirs::cache_dir().ok_or_else(|| "no cache dir".to_string())?;
    dest.push("bookos-store");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    dest.push(&filename);

    let total = head_size(&url).unwrap_or(0);
    let dest_clone = dest.clone();
    let op = op_id.clone().unwrap_or_default();

    let insecure = url_needs_insecure(&url)
        || std::env::var("BOOKOS_INSECURE_TLS").ok().as_deref() == Some("1");
    let mut cmd = Command::new("curl");
    cmd.args(["-fSL", "--silent"]);
    if insecure { cmd.arg("-k"); eprintln!("[bookos-store] download_pkg using -k for {}", url); }
    cmd.arg("-o").arg(&dest).arg(&url);
    let mut child = cmd.spawn().map_err(|e| format!("curl falló: {}", e))?;

    let pid = child.id();
    if let Some(id) = op_id.as_deref() { track(id, pid); }

    let stop_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let stop_clone = stop_flag.clone();
    let op_for_thread = op.clone();
    let poller = std::thread::spawn(move || {
        while !stop_clone.load(std::sync::atomic::Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(200));
            let cur = std::fs::metadata(&dest_clone).map(|m| m.len()).unwrap_or(0);
            let pct = if total > 0 { ((cur as f64 / total as f64) * 100.0).min(99.0) } else { 0.0 };
            if !op_for_thread.is_empty() {
                set_progress(&op_for_thread, pct, cur, total);
            }
        }
    });

    let status = child.wait().map_err(|e| e.to_string())?;
    stop_flag.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = poller.join();

    if !status.success() {
        if let Some(id) = op_id.as_deref() { untrack(id); }
        let _ = std::fs::remove_file(&dest);
        if status.code().is_none() { return Err("__cancelled__".into()); }
        return Err(format!("Descarga falló (exit {})", status.code().unwrap_or(-1)));
    }
    let final_size = std::fs::metadata(&dest).map(|m| m.len()).unwrap_or(0);
    if !op.is_empty() { set_progress(&op, 100.0, final_size, if total>0 { total } else { final_size }); }
    if let Some(id) = op_id.as_deref() { untrack(id); }
    Ok(dest.to_string_lossy().to_string())
}

/// Fetch latest release info for a single app from the BookOS catalog.
/// Frontend keeps calling `fetch_release({ repo })` — for back-compat the
/// `repo` argument is now treated as the package id (or, if it contains "/",
/// the last segment is used). Returns a GitHub-release-compatible shape so
/// the existing UI code keeps working unchanged.
#[tauri::command]
async fn fetch_release(repo: String) -> Result<serde_json::Value, String> {
    let pkg = repo.rsplit('/').next().unwrap_or(&repo).to_string();

    // Pull from already-cached catalog first (cheap, no extra HTTP).
    let apps = fetch_catalog_cached();
    let app = apps.into_iter().find(|a| a["pkg"].as_str() == Some(&pkg));
    let app = match app {
        Some(a) => a,
        None => return Err(format!("App '{}' no está en el catálogo.", pkg)),
    };

    // Map catalog shape → GitHub-release shape that main.js already consumes.
    let tag = app.get("available").cloned().unwrap_or(serde_json::Value::Null);
    let assets = app.get("assets").cloned().unwrap_or(serde_json::Value::Array(vec![]));
    let html_url = app.get("html_url").cloned().unwrap_or(serde_json::Value::String(String::new()));
    Ok(serde_json::json!({
        "tag_name": tag,
        "assets":   assets,
        "html_url": html_url,
        "body":     app.get("notes").cloned().unwrap_or(serde_json::Value::String(String::new())),
    }))
}

/// HEAD request via curl to discover Content-Length. Returns None on failure.
/// Tries with TLS verify first, then `-k` (matches catalog fetch behavior).
fn head_size(url: &str) -> Option<u64> {
    for insecure in [false, true] {
        let mut cmd = Command::new("curl");
        cmd.args(["-sIL", "-o", "/dev/null", "-w", "%{size_download}\n%{header_json}"]);
        if insecure { cmd.arg("-k"); }
        cmd.arg(url);
        let out = cmd.output().ok()?;
        if !out.status.success() { continue; }
        let s = String::from_utf8_lossy(&out.stdout);
        if let Some(line) = s.lines().nth(1) {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(line) {
                if let Some(cl) = v.get("content-length")
                    .and_then(|h| h.as_array())
                    .and_then(|a| a.last())
                    .and_then(|x| x.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
                {
                    return Some(cl);
                }
            }
        }
    }
    None
}

/// True if the URL works without TLS verification but not with it
/// (i.e. server cert is broken/self-signed/expired). Cached after first call.
fn url_needs_insecure(url: &str) -> bool {
    use std::sync::OnceLock;
    static MEMO: OnceLock<std::sync::Mutex<std::collections::HashMap<String, bool>>> = OnceLock::new();
    let host = url.split('/').nth(2).unwrap_or("").to_string();
    let memo = MEMO.get_or_init(|| std::sync::Mutex::new(std::collections::HashMap::new()));
    {
        if let Ok(g) = memo.lock() {
            if let Some(&v) = g.get(&host) { return v; }
        }
    }
    // Try strict TLS first.
    let strict_ok = Command::new("curl")
        .args(["-fsI", "--max-time", "5", "-o", "/dev/null"])
        .arg(url).status().map(|s| s.success()).unwrap_or(false);
    let needs = !strict_ok;
    if let Ok(mut g) = memo.lock() { g.insert(host, needs); }
    needs
}

#[tauri::command]
fn detect_system_theme() -> String {
    let kde = [
        ("kreadconfig6", &["--group", "General", "--key", "ColorScheme"][..]),
        ("kreadconfig5", &["--group", "General", "--key", "ColorScheme"][..]),
    ];
    for (bin, args) in kde {
        if let Ok(out) = Command::new(bin).args(args).output() {
            let s = String::from_utf8_lossy(&out.stdout).to_lowercase();
            if s.contains("dark") { return "dark".into(); }
            if s.contains("light") { return "light".into(); }
        }
    }
    if let Ok(out) = Command::new("gsettings")
        .args(["get", "org.gnome.desktop.interface", "color-scheme"]).output()
    {
        let s = String::from_utf8_lossy(&out.stdout).to_lowercase();
        if s.contains("dark") { return "dark".into(); }
        if s.contains("light") { return "light".into(); }
    }
    "auto".into()
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            list_apps,
            clear_catalog_cache,
            is_installed,
            pm_info,
            get_icon,
            verify_password,
            launch_app,
            open_release_page,
            install_pkg_file,
            install_pkg_by_name,
            upgrade_all,
            uninstall_pkg,
            download_pkg,
            cancel_op,
            progress,
            fetch_release,
            detect_system_theme,
        ])
        .setup(|app| {
            use tauri::{WebviewWindowBuilder, WebviewUrl};
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::App("index.html".into()))
                .title("Bookos Store")
                .inner_size(960.0, 680.0)
                .min_inner_size(640.0, 480.0)
                .decorations(false)
                .transparent(true)
                .resizable(true)
                .visible(false)
                .initialization_script(include_str!("../tauri-bridge.js"))
                .build()?;
            let _ = win.show();
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running app");
}
