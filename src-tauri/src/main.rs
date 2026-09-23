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

    // Nunca se degrada TLS por cuenta propia: el catálogo decide qué URL y qué
    // sha256 llegan al instalador privilegiado, así que un reintento con `-k`
    // ante un certificado inválido entregaba ambos a quien estuviera en medio.
    // `-k` solo con opt-in explícito (BOOKOS_INSECURE_TLS=1), como download_pkg.
    let insecure = std::env::var("BOOKOS_INSECURE_TLS").ok().as_deref() == Some("1");
    for candidate in std::iter::once(url.as_str()).chain(fallback_url.as_deref()) {
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
    // Una sola consulta al gestor de paquetes para todo el catálogo. Antes se
    // lanzaba un proceso por app, en serie, y la rejilla no se pintaba hasta
    // que terminaban los ~30 fork+exec.
    let installed_map = installed_versions(pm);
    let enriched: Vec<serde_json::Value> = apps.into_iter().map(|mut a| {
        let pkg = a["pkg"].as_str().unwrap_or("").to_string();
        let installed = lookup_installed(&installed_map, &pkg);
        a["installed"] = serde_json::json!(installed);
        a
    }).collect();
    serde_json::json!(enriched)
}

/// Volcado completo de paquetes instalados → `{nombre: versión}`, en **una**
/// invocación del gestor. Listar todo y filtrar en memoria sale más barato que
/// preguntar paquete a paquete, y evita las rarezas de código de salida que
/// tiene `pacman -Q` cuando alguno de los nombres no está instalado.
fn installed_versions(pm: Pm) -> HashMap<String, String> {
    let out = match pm {
        Pm::Pacman => Command::new("pacman").arg("-Q").output(),
        Pm::Dnf    => Command::new("rpm").args(["-qa", "--qf", "%{NAME} %{VERSION}-%{RELEASE}\n"]).output(),
        Pm::Apt    => Command::new("dpkg-query").args(["-W", "-f=${Package} ${Version}\n"]).output(),
    };
    let mut map = HashMap::new();
    if let Ok(o) = out {
        if o.status.success() {
            for line in String::from_utf8_lossy(&o.stdout).lines() {
                let mut it = line.split_whitespace();
                if let (Some(name), Some(ver)) = (it.next(), it.next()) {
                    map.insert(name.to_string(), ver.to_string());
                }
            }
        }
    }
    map
}

/// Versión instalada de `pkg` según el volcado, con el mismo respaldo que antes:
/// si el gestor no lo conoce pero el binario está en disco, se marca `manual`.
fn lookup_installed(map: &HashMap<String, String>, pkg: &str) -> Option<String> {
    if let Some(v) = map.get(pkg) { return Some(v.clone()); }
    let bin_exists = ["/usr/bin", "/usr/local/bin", "/bin"]
        .iter()
        .any(|d| std::path::Path::new(&format!("{}/{}", d, pkg)).exists());
    if bin_exists { Some(MANUAL_VER.to_string()) } else { None }
}

/// Sentinel returned when the app binary exists on disk but the package manager
/// has no record of it (installed via AppImage, manual copy, or a different
/// package name). Treated as "installed, unknown version" by the frontend so it
/// always offers a clean reinstall to register it properly.
const MANUAL_VER: &str = "manual";

fn pkg_version(pm: Pm, pkg: &str) -> Option<String> {
    let from_pm = match pm {
        Pm::Pacman => {
            Command::new("pacman").args(["-Q", pkg]).output().ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    s.split_whitespace().nth(1).map(|v| v.to_string())
                })
        }
        Pm::Dnf => {
            // Exit code is the source of truth: 0 = installed, nonzero = not.
            // A real version starts with a digit — guards against localized
            // error text ("no está instalado") leaking into stdout.
            Command::new("rpm")
                .args(["-q", "--qf", "%{VERSION}-%{RELEASE}", pkg])
                .output().ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() || !s.chars().next().map(|c| c.is_ascii_digit()).unwrap_or(false) {
                        None
                    } else { Some(s) }
                })
        }
        Pm::Apt => {
            Command::new("dpkg-query").args(["-W", "-f=${Version}", pkg]).output().ok()
                .filter(|o| o.status.success())
                .and_then(|o| {
                    let s = String::from_utf8_lossy(&o.stdout).trim().to_string();
                    if s.is_empty() { None } else { Some(s) }
                })
        }
    };
    if from_pm.is_some() { return from_pm; }

    // Fallback: binary present but not tracked by the package manager.
    let bin_exists = ["/usr/bin", "/usr/local/bin", "/bin"]
        .iter()
        .any(|d| std::path::Path::new(&format!("{}/{}", d, pkg)).exists());
    if bin_exists { Some(MANUAL_VER.to_string()) } else { None }
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

/// Nombre de paquete plausible: alfanumérico + separadores habituales, sin
/// empezar por '-' (inyectaría flags en pacman/dnf/apt, que corren como root)
/// y sin '/' (launch_app no debe ejecutar rutas arbitrarias).
fn valid_pkg_name(p: &str) -> bool {
    !p.is_empty()
        && p.len() <= 128
        && !p.starts_with('-')
        && p.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_' | '+' | '@' | ':'))
}

#[tauri::command]
fn launch_app(pkg: String) -> Result<(), String> {
    if !valid_pkg_name(&pkg) {
        return Err(format!("Nombre de paquete inválido: {}", pkg));
    }
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
    if !valid_pkg_name(&pkg) {
        return Err(format!("Nombre de paquete inválido: {}", pkg));
    }
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
    if !valid_pkg_name(&pkg) {
        return Err(format!("Nombre de paquete inválido: {}", pkg));
    }
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

/// Comprueba el sha256 del archivo descargado contra el que publica el catálogo.
/// Falla cerrado: si no coincide, el llamante borra el archivo y no se instala.
///
/// Esto detecta corrupción y manipulación en tránsito, pero no protege contra un
/// servidor comprometido (el hash viaja por el mismo canal que el paquete). La
/// defensa real es firmar los paquetes con la clave GPG de release, que ya existe
/// pero que hoy solo se aplica a las ISO.
fn verify_sha256(path: &std::path::Path, expected: &str) -> Result<(), String> {
    let expected = expected.trim().to_lowercase();
    if expected.len() != 64 || !expected.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("El catálogo no trae un sha256 válido para este paquete.".into());
    }
    let out = Command::new("sha256sum")
        .arg(path)
        .output()
        .map_err(|e| format!("No se pudo calcular el hash del paquete: {}", e))?;
    if !out.status.success() {
        return Err("No se pudo calcular el hash del paquete.".into());
    }
    let got = String::from_utf8_lossy(&out.stdout)
        .split_whitespace()
        .next()
        .unwrap_or("")
        .to_lowercase();
    if got != expected {
        return Err(format!(
            "El paquete descargado no coincide con el catálogo (sha256 esperado {}…, obtenido {}…). \
             Se ha descartado la descarga.",
            &expected[..12],
            if got.len() >= 12 { &got[..12] } else { "desconocido" }
        ));
    }
    Ok(())
}

/// Download asset to ~/.cache/bookos-store/. Writes progress to OPS map for
/// frontend to poll via `progress(op_id)`. Cancellable via cancel_op(op_id).
/// El paquete se verifica contra el sha256 del catálogo antes de devolverlo.
#[tauri::command]
async fn download_pkg(url: String, filename: String, sha256: Option<String>, op_id: Option<String>) -> Result<String, String> {
    let mut dest = dirs::cache_dir().ok_or_else(|| "no cache dir".to_string())?;
    dest.push("bookos-store");
    std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;

    // `filename` viene del catálogo remoto (asset.name), no es de confianza.
    // Aceptar solo un basename limpio para que la escritura no pueda salir de
    // ~/.cache/bookos-store/ (evita path traversal / escritura arbitraria).
    let safe_name = std::path::Path::new(&filename)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| *n == filename && !n.is_empty() && *n != "." && *n != "..")
        .ok_or_else(|| format!("nombre de archivo inválido: {}", filename))?;
    dest.push(safe_name);

    let total = head_size(&url).unwrap_or(0);
    let dest_clone = dest.clone();
    let op = op_id.clone().unwrap_or_default();

    // No degradar TLS automáticamente: solo aceptar certificados inválidos con
    // opt-in explícito por variable de entorno (nunca de forma silenciosa/memoizada).
    let insecure = std::env::var("BOOKOS_INSECURE_TLS").ok().as_deref() == Some("1");
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
    // Integridad antes de entregar la ruta: si el paquete no es el que el
    // catálogo dice, se borra y no llega nunca al instalador privilegiado.
    // Sin sha256 no hay nada que comparar, y el paquete iría igual a root:
    // se rechaza en vez de instalarlo a ciegas.
    let verified = match sha256.as_deref().map(str::trim).filter(|s| !s.is_empty()) {
        Some(exp) => verify_sha256(&dest, exp),
        None => Err(format!("El catálogo no trae sha256 para {}; no se instala sin verificar", safe_name)),
    };
    if let Err(e) = verified {
        let _ = std::fs::remove_file(&dest);
        if let Some(id) = op_id.as_deref() { untrack(id); }
        eprintln!("[bookos-store] verificación sha256 fallida para {}: {}", safe_name, e);
        return Err(e);
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
fn head_size(url: &str) -> Option<u64> {
    let insecure = std::env::var("BOOKOS_INSECURE_TLS").ok().as_deref() == Some("1");
    let mut cmd = Command::new("curl");
    cmd.args(["-sIL", "-o", "/dev/null", "-w", "%{size_download}\n%{header_json}"]);
    if insecure { cmd.arg("-k"); }
    cmd.arg(url);
    let out = cmd.output().ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout);
    let line = s.lines().nth(1)?;
    let v = serde_json::from_str::<serde_json::Value>(line).ok()?;
    v.get("content-length")
        .and_then(|h| h.as_array())
        .and_then(|a| a.last())
        .and_then(|x| x.as_str())
        .and_then(|s| s.parse::<u64>().ok())
}


/// El tema del escritorio, preguntado al portal XDG.
///
/// Va **antes** que `kreadconfig6`: esa orden solo sabe de Plasma y lee el
/// `kdeglobals` del usuario, que en una sesión BookOS no lo escribe nadie —el
/// tema se elige en la tarjeta de Apariencia y vive en `panel.conf`—, así que
/// la app se quedaba con el tema con el que se instaló el sistema. El portal
/// lo sirve la sesión que esté corriendo: BookOS desde el propio compositor,
/// y Plasma y GNOME también, de modo que esto funciona en las tres.
///
/// `color-scheme`: 1 oscuro, 2 claro, 0 sin preferencia. `gdbus` viene con
/// GLib, que ya es dependencia de cualquier app Tauri: no añade nada.
fn portal_color_scheme() -> Option<String> {
    let salida = std::process::Command::new("gdbus")
        .args([
            "call",
            "--session",
            "--dest",
            "org.freedesktop.portal.Desktop",
            "--object-path",
            "/org/freedesktop/portal/desktop",
            "--method",
            "org.freedesktop.portal.Settings.ReadOne",
            "org.freedesktop.appearance",
            "color-scheme",
        ])
        .output()
        .ok()?;
    // Contesta «(<uint32 1>,)»; el 0 es «sin preferencia» y no es respuesta.
    let texto = String::from_utf8_lossy(&salida.stdout);
    let n = texto.split("uint32").nth(1)?.trim_start().chars().next()?;
    match n {
        '1' => Some("dark".to_string()),
        '2' => Some("light".to_string()),
        _ => None,
    }
}

#[tauri::command]
fn detect_system_theme() -> String {
    if let Some(tema) = portal_color_scheme() {
        return tema;
    }
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

/// Quita el zoom de página que WebKitGTK hace con el pellizco del touchpad.
///
/// No hay ajuste para eso ni en WebKit ni en Tauri, y la página no se entera:
/// el pellizco lo consume un `GtkGestureZoom` de la propia vista y llega como
/// `setMagnification`, no como `wheel` ni `touch*`, así que ningún
/// `preventDefault` lo frena. Se apaga ese gesto y ningún otro: con fase `None`
/// GTK deja de pasarle eventos (gtkeventcontroller.c, 3.24). Va en
/// `on_page_load` para cubrir también las ventanas que se abran después.
fn desactivar_zoom_por_pellizco<R: tauri::Runtime>(
    webview: &tauri::Webview<R>,
    _: &tauri::webview::PageLoadPayload<'_>,
) {
    #[cfg(target_os = "linux")]
    let _ = webview.with_webview(|webview| {
        use gtk::glib::translate::from_glib_none;
        use gtk::prelude::*;

        // WebKitGTK guarda ahí el gesto (WebKitWebViewBase.cpp, 2.52.5). No es
        // API pública: si la clave cambia, no se encuentra y el zoom vuelve.
        // Lo guardado es un puntero C a un GObject, no un tipo de Rust.
        let Some(gesto) = (unsafe { webview.inner().data::<gtk::ffi::GtkGesture>("wk-view-zoom-gesture") }) else {
            return;
        };
        let gesto: gtk::Gesture = unsafe { from_glib_none(gesto.as_ptr()) };
        gesto.set_propagation_phase(gtk::PropagationPhase::None);
    });
}

fn main() {
    tauri::Builder::default()
        .on_page_load(desactivar_zoom_por_pellizco)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            bookos_palette_css,
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

// ── Color dinámico ───────────────────────────────────────────────────────
// La paleta la genera BookOS Settings desde el fondo de pantalla y la deja en
// ~/.config/bookos/palette.css. Aquí solo se lee: quien tiñe es la hoja, que
// el cliente bookos-palette.js inyecta al final de <head>.
#[tauri::command]
fn bookos_palette_css() -> String {
    std::env::var("HOME").ok()
        .map(|h| std::path::Path::new(&h).join(".config/bookos/palette.css"))
        .and_then(|p| std::fs::read_to_string(p).ok())
        .unwrap_or_default()
}

