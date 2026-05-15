#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::process::Command;

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

#[tauri::command]
fn list_apps() -> serde_json::Value {
    let apps = catalog();
    let enriched: Vec<serde_json::Value> = apps.into_iter().map(|mut a| {
        let pkg = a["pkg"].as_str().unwrap_or("");
        let installed = pacman_version(pkg);
        a["installed"] = serde_json::json!(installed);
        a
    }).collect();
    serde_json::json!(enriched)
}

fn pacman_version(pkg: &str) -> Option<String> {
    let out = Command::new("pacman").args(["-Q", pkg]).output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    s.split_whitespace().nth(1).map(|v| v.to_string())
}

#[tauri::command]
fn is_installed(pkg: String) -> bool {
    pacman_version(&pkg).is_some()
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

/// Install a .pkg.tar.zst file via pkexec pacman -U.
/// Frontend handles GitHub Releases fetch + download; here we just install
/// from a local file path.
#[tauri::command]
async fn install_pkg_file(path: String) -> Result<String, String> {
    if !std::path::Path::new(&path).exists() {
        return Err(format!("Archivo no existe: {}", path));
    }
    // Prefer pkexec for GUI password prompt; fallback to sudo if missing.
    let bin = if Command::new("which").arg("pkexec").output()
        .map(|o| o.status.success()).unwrap_or(false) { "pkexec" } else { "sudo" };
    let out = Command::new(bin)
        .args(["pacman", "-U", "--noconfirm", &path])
        .output()
        .map_err(|e| format!("Fallo al lanzar {}: {}", bin, e))?;
    let stdout = String::from_utf8_lossy(&out.stdout).to_string();
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(format!("Instalación falló: {}\n{}", stderr.lines().last().unwrap_or(""), stdout.lines().last().unwrap_or("")));
    }
    Ok(stdout)
}

/// Uninstall a package via pkexec pacman -Rs.
#[tauri::command]
async fn uninstall_pkg(pkg: String) -> Result<String, String> {
    let bin = if Command::new("which").arg("pkexec").output()
        .map(|o| o.status.success()).unwrap_or(false) { "pkexec" } else { "sudo" };
    let out = Command::new(bin)
        .args(["pacman", "-Rs", "--noconfirm", &pkg])
        .output()
        .map_err(|e| format!("Fallo al lanzar {}: {}", bin, e))?;
    let stderr = String::from_utf8_lossy(&out.stderr).to_string();
    if !out.status.success() {
        return Err(format!("Desinstalación falló: {}", stderr.lines().last().unwrap_or("error")));
    }
    Ok(String::from("ok"))
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
