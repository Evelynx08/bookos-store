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

/// BookOS app catalog. Each app has: pkgname, label, description, category,
/// GitHub repo for releases, optional icon URL.
fn catalog() -> Vec<serde_json::Value> {
    vec![
        serde_json::json!({
            "pkg": "bookos-notepad",
            "label": "Bloc de notas",
            "description": "Editor de texto con pestañas, preview HTML/MD, adblock y atajos personalizables.",
            "category": "Productividad",
            "repo": "Evelynx08/bookos-notepad",
            "icon": "bookos-notepad",
            "accent": "#0A6FDC"
        }),
        serde_json::json!({
            "pkg": "bookos-calc",
            "label": "Calculadora",
            "description": "Calculadora estándar, científica, conversor de unidades y monedas.",
            "category": "Utilidades",
            "repo": "Evelynx08/bookos-calc",
            "icon": "bookos-calc",
            "accent": "#33D878"
        }),
        serde_json::json!({
            "pkg": "bookos-clock",
            "label": "Reloj",
            "description": "Reloj mundial, alarmas, temporizador y cronómetro.",
            "category": "Utilidades",
            "repo": "Evelynx08/bookos-clock",
            "icon": "bookos-clock",
            "accent": "#273042"
        }),
        serde_json::json!({
            "pkg": "bookos-settings",
            "label": "Ajustes",
            "description": "Centro de control y configuración del sistema.",
            "category": "Sistema",
            "repo": "Evelynx08/bookos-settings",
            "icon": "bookos-settings",
            "accent": "#8e8e93"
        }),
        serde_json::json!({
            "pkg": "bookos-store",
            "label": "Bookos Store",
            "description": "Tienda de apps de BookOS. Actualízala desde aquí.",
            "category": "Sistema",
            "repo": "Evelynx08/bookos-store",
            "icon": "bookos-store",
            "accent": "#9C7BFF",
            "self": true
        }),
    ]
}

/// Package manager backend detected at runtime.
#[derive(Clone, Copy, PartialEq)]
enum Pm { Pacman, Dnf }

fn detect_pm() -> Pm {
    let has = |bin: &str| Command::new("which").arg(bin).output()
        .map(|o| o.status.success()).unwrap_or(false);
    if has("dnf") || has("rpm") { Pm::Dnf } else { Pm::Pacman }
}

#[tauri::command]
fn list_apps() -> serde_json::Value {
    let pm = detect_pm();
    let apps = catalog();
    let enriched: Vec<serde_json::Value> = apps.into_iter().map(|mut a| {
        let pkg = a["pkg"].as_str().unwrap_or("");
        let installed = pkg_version(pm, pkg);
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
    }
}

#[tauri::command]
fn launch_app(pkg: String) -> Result<(), String> {
    // Tauri-built apps install their binary at /usr/bin/<pkgname>
    Command::new(&pkg).spawn().map(|_| ())
        .map_err(|e| format!("No se pudo lanzar {}: {}", pkg, e))
}

/// Open release page in browser as fallback for manual install
#[tauri::command]
fn open_release_page(repo: String) -> Result<(), String> {
    let url = format!("https://github.com/{}/releases", repo);
    Command::new("xdg-open").arg(&url).spawn().map(|_| ())
        .map_err(|e| e.to_string())
}

/// Run privileged command. If `password` provided, uses `sudo -S` and feeds it
/// via stdin. If empty, falls back to pkexec. If `op_id` set, registers PID for
/// cancellation.
fn run_priv(args: &[&str], password: &str, op_id: Option<&str>) -> Result<String, String> {
    if password.is_empty() {
        let bin = if Command::new("which").arg("pkexec").output()
            .map(|o| o.status.success()).unwrap_or(false) { "pkexec" } else { "sudo" };
        let child = Command::new(bin).args(args)
            .stdout(Stdio::piped()).stderr(Stdio::piped())
            .spawn().map_err(|e| format!("Fallo al lanzar {}: {}", bin, e))?;
        if let Some(id) = op_id { track(id, child.id()); }
        let out = child.wait_with_output().map_err(|e| e.to_string())?;
        if let Some(id) = op_id { untrack(id); }
        if !out.status.success() {
            let stderr = String::from_utf8_lossy(&out.stderr);
            return Err(format!("Falló: {}", stderr.lines().last().unwrap_or("error")));
        }
        return Ok(String::from_utf8_lossy(&out.stdout).to_string());
    }
    let mut child = Command::new("sudo")
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
        return Err(format!("Falló: {}", stderr.lines().last().unwrap_or(stdout.lines().last().unwrap_or("error"))));
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
    if !std::path::Path::new(&path).exists() {
        return Err(format!("Archivo no existe: {}", path));
    }
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-U", "--noconfirm", &path],
        Pm::Dnf => vec!["dnf", "install", "-y", &path],
    };
    run_priv(&args, password.as_deref().unwrap_or(""), op_id.as_deref())
}

/// Uninstall a package via pacman -Rs or dnf remove.
#[tauri::command]
async fn uninstall_pkg(pkg: String, password: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-Rs", "--noconfirm", &pkg],
        Pm::Dnf => vec!["dnf", "remove", "-y", &pkg],
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

    let mut child = Command::new("curl")
        .args(["-fSL", "--silent", "-o"])
        .arg(&dest)
        .arg(&url)
        .spawn().map_err(|e| format!("curl falló: {}", e))?;

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

/// HEAD request via curl to discover Content-Length. Returns None on failure.
fn head_size(url: &str) -> Option<u64> {
    let out = Command::new("curl")
        .args(["-sIL", "-o", "/dev/null", "-w", "%{size_download}\n%{header_json}"])
        .arg(url).output().ok()?;
    let s = String::from_utf8_lossy(&out.stdout);
    let v: serde_json::Value = serde_json::from_str(s.lines().nth(1)?).ok()?;
    let cl = v.get("content-length")?.as_array()?.last()?.as_str()?.parse::<u64>().ok()?;
    Some(cl)
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
    const PREFERRED_PORT: u16 = 17842;
    let port = if portpicker::is_free_tcp(PREFERRED_PORT) {
        PREFERRED_PORT
    } else {
        portpicker::pick_unused_port().unwrap_or(1431)
    };
    let url = format!("http://localhost:{}", port).parse().unwrap();

    tauri::Builder::default()
        .plugin(tauri_plugin_localhost::Builder::new(port).build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            list_apps,
            is_installed,
            pm_info,
            get_icon,
            verify_password,
            launch_app,
            open_release_page,
            install_pkg_file,
            uninstall_pkg,
            download_pkg,
            cancel_op,
            progress,
            detect_system_theme,
        ])
        .setup(move |app| {
            use tauri::{WebviewWindowBuilder, WebviewUrl};
            let win = WebviewWindowBuilder::new(app, "main", WebviewUrl::External(url))
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
