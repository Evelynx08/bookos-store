#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::{Command, Stdio};
use std::io::Write;
use base64::{Engine as _, engine::general_purpose};

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
            "accent": "#0a84ff"
        }),
        serde_json::json!({
            "pkg": "bookos-calc",
            "label": "Calculadora",
            "description": "Calculadora estándar, científica, conversor de unidades y monedas.",
            "category": "Utilidades",
            "repo": "Evelynx08/bookos-calc",
            "icon": "bookos-calc",
            "accent": "#ff9500"
        }),
        serde_json::json!({
            "pkg": "bookos-clock",
            "label": "Reloj",
            "description": "Reloj mundial, alarmas, temporizador y cronómetro.",
            "category": "Utilidades",
            "repo": "Evelynx08/bookos-clock",
            "icon": "bookos-clock",
            "accent": "#ff3b30"
        }),
        serde_json::json!({
            "pkg": "bookos-ai",
            "label": "AI",
            "description": "Asistente de IA local para BookOS.",
            "category": "Productividad",
            "repo": "Evelynx08/bookos-ai",
            "icon": "bookos-ai",
            "accent": "#af52de"
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
            "accent": "#0a84ff",
            "self": true
        }),
        serde_json::json!({
            "pkg": "bookos-launchpad",
            "label": "Launchpad",
            "description": "Lanzador de aplicaciones estilo iPad.",
            "category": "Sistema",
            "repo": "Evelynx08/BookOS-Launchpad",
            "icon": "bookos-launchpad",
            "accent": "#34c759"
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
/// via stdin (lets the UI render a custom password dialog). If empty, falls
/// back to pkexec (system polkit dialog).
fn run_priv(args: &[&str], password: &str) -> Result<String, String> {
    if password.is_empty() {
        let bin = if Command::new("which").arg("pkexec").output()
            .map(|o| o.status.success()).unwrap_or(false) { "pkexec" } else { "sudo" };
        let out = Command::new(bin).args(args).output()
            .map_err(|e| format!("Fallo al lanzar {}: {}", bin, e))?;
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
    if let Some(mut s) = child.stdin.take() {
        let _ = s.write_all(format!("{}\n", password).as_bytes());
    }
    let out = child.wait_with_output().map_err(|e| e.to_string())?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        if stderr.contains("incorrect password") || stderr.contains("Sorry, try again") {
            return Err("Contraseña incorrecta".into());
        }
        return Err(format!("Falló: {}", stderr.lines().last().unwrap_or(stdout.lines().last().unwrap_or("error"))));
    }
    Ok(stdout)
}

/// Install a local package file. Backend picks pacman -U or dnf install based
/// on detected distro. If `password` is provided, uses sudo -S (custom dialog);
/// otherwise pkexec (system dialog).
#[tauri::command]
async fn install_pkg_file(path: String, password: Option<String>) -> Result<String, String> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("Archivo no existe: {}", path));
    }
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-U", "--noconfirm", &path],
        Pm::Dnf => vec!["dnf", "install", "-y", &path],
    };
    run_priv(&args, password.as_deref().unwrap_or(""))
}

/// Uninstall a package via pacman -Rs or dnf remove.
#[tauri::command]
async fn uninstall_pkg(pkg: String, password: Option<String>) -> Result<String, String> {
    let args: Vec<&str> = match detect_pm() {
        Pm::Pacman => vec!["pacman", "-Rs", "--noconfirm", &pkg],
        Pm::Dnf => vec!["dnf", "remove", "-y", &pkg],
    };
    run_priv(&args, password.as_deref().unwrap_or(""))?;
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

/// Download a release asset URL to ~/.cache/bookos-store/ and return local path.
#[tauri::command]
async fn download_pkg(url: String, filename: String) -> Result<String, String> {
    let mut dest = dirs::cache_dir().ok_or_else(|| "no cache dir".to_string())?;
    dest.push("bookos-store");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
    dest.push(&filename);
    // Use curl (always available on Arch) for download
    let out = Command::new("curl")
        .args(["-fSL", "-o"])
        .arg(&dest)
        .arg(&url)
        .output()
        .map_err(|e| format!("curl falló: {}", e))?;
    if !out.status.success() {
        let stderr = String::from_utf8_lossy(&out.stderr);
        return Err(format!("Descarga falló: {}", stderr.lines().last().unwrap_or("")));
    }
    Ok(dest.to_string_lossy().to_string())
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
