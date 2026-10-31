#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod license;
mod offline;
mod provider;
mod selection;
use provider::Settings;
use serde::Deserialize;
use serde_json::{json, Value};
use std::{
    io::{BufRead, BufReader},
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tauri::{
    menu::{Menu, MenuItem},
    tray::TrayIconBuilder,
    Emitter, Manager, State,
};
use tauri_plugin_autostart::ManagerExt;

struct Core {
    settings: Mutex<Settings>,
    client: reqwest::Client,
    tracker: Mutex<selection::SelectionTracker>,
    paused: AtomicBool,
    permission: AtomicBool,
    child: Mutex<Option<std::process::Child>>,
    popup_ready: AtomicBool,
    pending_popup: Mutex<Option<Value>>,
    popup_epoch: AtomicU64,
    cloud_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    status_error: Mutex<Option<String>>,
    diagnostics: Mutex<Value>,
}
// Opt-in QA counters contain no source text, translation, key, app names or history.
#[cfg(debug_assertions)]
fn qa(app: &tauri::AppHandle, core: &Arc<Core>, event: &str) {
    if !std::env::args().any(|a| a == "--qa-diagnostics") {
        return;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;
    let mut state = core.diagnostics.lock().unwrap();
    let key = format!("{}_count", event);
    let count = state[&key].as_u64().unwrap_or(0) + 1;
    state[&key] = json!(count);
    state["last_event"] = json!(event);
    state["last_event_ms"] = json!(now);
    state[format!("{}_at_ms", event)] = json!(now);
    state["permission"] = json!(core.permission.load(Ordering::SeqCst));
    state["paused"] = json!(core.paused.load(Ordering::SeqCst));
    state["popup_renderer_ready"] = json!(core.popup_ready.load(Ordering::SeqCst));
    state["automatic"] = json!(core.settings.lock().unwrap().consent);
    if let Ok(root) = app.path().app_cache_dir() {
        let _ = std::fs::write(
            root.join("qa-state.json"),
            serde_json::to_vec_pretty(&*state).unwrap(),
        );
    }
}
#[cfg(not(debug_assertions))]
fn qa(_app: &tauri::AppHandle, _core: &Arc<Core>, _event: &str) {}
fn secret(name: &str) -> Result<keyring::Entry, String> {
    keyring::Entry::new("app.claro.translate", name)
        .map_err(|_| "Secure credential storage is unavailable.".into())
}
fn key_for(s: &Settings) -> Result<String, String> {
    use sha2::{Digest, Sha256};
    let account = format!(
        "provider-{}",
        hex::encode(Sha256::digest(s.base_url.trim_end_matches('/').as_bytes()))
    );
    secret(&account)?
        .get_password()
        .map_err(|_| "Add your provider API key in Settings.".into())
}
fn licensed() -> bool {
    license::cached().is_some()
}
fn control(core: &Arc<Core>) {
    use std::io::Write;
    let s = core.settings.lock().unwrap().clone();
    if let Some(child) = core.child.lock().unwrap().as_mut() {
        if let Some(input) = child.stdin.as_mut() {
            let enabled = s.consent && !core.paused.load(Ordering::SeqCst);
            let _ = writeln!(
                input,
                "{}\t{}",
                if enabled { "1" } else { "0" },
                s.excluded_apps.join("\t")
            );
        }
    }
}
fn config_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_config_dir().unwrap().join("settings.json")
}
#[tauri::command]
fn get_state(app: tauri::AppHandle, core: State<Arc<Core>>) -> Value {
    let r = license::release();
    let settings = core.settings.lock().unwrap().clone();
    let has_key = key_for(&settings).is_ok();
    json!({"settings":settings,"hasKey":has_key,"licensed":licensed(),"paused":core.paused.load(Ordering::SeqCst),"permission":core.permission.load(Ordering::SeqCst),"checkoutAvailable":!r.checkout_url.is_empty(),"modelAvailable":r.model.is_some(),"launchAtLogin":app.autolaunch().is_enabled().unwrap_or(false),"statusError":core.status_error.lock().unwrap().clone()})
}
#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    settings: Settings,
    api_key: Option<String>,
) -> Result<(), String> {
    provider::endpoint(&settings.base_url)?;
    if settings.model.trim().is_empty() || settings.model.len() > 200 {
        return Err("Enter a provider model name.".into());
    }
    if !["cloud", "offline"].contains(&settings.mode.as_str()) {
        return Err("Choose cloud or offline translation.".into());
    }
    if settings.source_language != "Auto" && !LANGUAGES.contains(&settings.source_language.as_str())
    {
        return Err("Choose a supported source language.".into());
    }
    if !LANGUAGES.contains(&settings.target.as_str()) {
        return Err("Choose a supported target language.".into());
    }
    if settings.excluded_apps.len() > 50
        || settings
            .excluded_apps
            .iter()
            .any(|s| s.len() > 256 || s.contains(['\t', '\n', '\r']))
    {
        return Err("Use up to 50 app identifiers, one per line.".into());
    }
    if let Some(key) = api_key {
        if !key.trim().is_empty() {
            {
                use sha2::{Digest, Sha256};
                secret(&format!(
                    "provider-{}",
                    hex::encode(Sha256::digest(
                        settings.base_url.trim_end_matches('/').as_bytes()
                    ))
                ))?
            }
            .set_password(key.trim())
            .map_err(|_| "Couldn't save your key securely.")?;
        }
    }
    let path = config_path(&app);
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|_| "Couldn't create settings folder.")?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec(&settings).unwrap())
        .map_err(|_| "Couldn't save settings.")?;
    std::fs::rename(temp, path).map_err(|_| "Couldn't finish saving settings.")?;
    *core.settings.lock().unwrap() = settings.clone();
    offline::configure(app.clone(), settings);
    control(&core);
    core.tracker.lock().unwrap().clear();
    dismiss(&app, &core, false);
    Ok(())
}
#[tauri::command]
fn remove_key(core: State<Arc<Core>>) -> Result<(), String> {
    use sha2::{Digest, Sha256};
    let s = core.settings.lock().unwrap();
    let name = format!(
        "provider-{}",
        hex::encode(Sha256::digest(s.base_url.trim_end_matches('/').as_bytes()))
    );
    match secret(&name)?.delete_credential() {
        Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
        Err(_) => Err("Couldn't remove the saved key.".into()),
    }
}
#[tauri::command]
fn set_paused(app: tauri::AppHandle, core: State<Arc<Core>>, paused: bool) {
    core.paused.store(paused, Ordering::SeqCst);
    core.tracker.lock().unwrap().clear();
    control(&core);
    dismiss(&app, &core, false);
    let _ = app.emit("state-changed", ());
}
fn abort_cloud(core: &Arc<Core>) {
    if let Some(task) = core.cloud_task.lock().unwrap().take() {
        task.abort();
    }
}
fn dismiss(app: &tauri::AppHandle, core: &Arc<Core>, remember: bool) {
    qa(app, core, if remember { "dismiss" } else { "clear" });
    abort_cloud(core);
    core.tracker.lock().unwrap().dismiss(remember);
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.hide();
    }
    *core.pending_popup.lock().unwrap() = None;
    let _ = app.emit_to("popup", "translation", json!({"state":"empty"}));
    let epoch = core.popup_epoch.fetch_add(1, Ordering::SeqCst) + 1;
    let app = app.clone();
    let core = core.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(10)).await;
        if core.popup_epoch.load(Ordering::SeqCst) == epoch
            && core.pending_popup.lock().unwrap().is_none()
        {
            core.popup_ready.store(false, Ordering::SeqCst);
            if let Some(w) = app.get_webview_window("popup") {
                let _ = w.destroy();
            }
        }
    });
}
#[tauri::command]
fn dismiss_popup(app: tauri::AppHandle, core: State<Arc<Core>>) {
    dismiss(&app, &core, true)
}
#[tauri::command]
async fn translate_manual(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    text: String,
) -> Result<String, String> {
    let core = core.inner().clone();
    let mut s = core.settings.lock().unwrap().clone();
    // The explicit Translate action authorizes this manual request only.
    s.consent = true;
    if s.mode == "offline" {
        return offline::translate(&app, &s, &text).await;
    }
    provider::translate(&core.client, &s, &key_for(&s)?, &text).await
}
#[tauri::command]
async fn activate_license(
    app: tauri::AppHandle,
    core: State<'_, Arc<Core>>,
    license_key: String,
) -> Result<(), String> {
    license::validate(&core.client, license_key.trim())
        .await
        .map_err(|e| e.message)?;
    license::save(license_key.trim())?;
    offline::configure(app, core.settings.lock().unwrap().clone());
    Ok(())
}
#[tauri::command]
async fn download_model(app: tauri::AppHandle, core: State<'_, Arc<Core>>) -> Result<(), String> {
    if !licensed() {
        return Err("Activate your offline license first.".into());
    }
    offline::download(&app, &core.client).await?;
    offline::configure(app, core.settings.lock().unwrap().clone());
    Ok(())
}
#[tauri::command]
fn open_purchase() -> Result<(), String> {
    let u = license::release().checkout_url;
    if u.is_empty() {
        return Err("Checkout is not configured in this development build.".into());
    }
    let url = url::Url::parse(&u).map_err(|_| "Invalid checkout URL.")?;
    if url.scheme() != "https" || url.host_str() != Some("polar.sh") {
        return Err("Checkout must use the configured Polar page.".into());
    }
    open_url(&u)
}
#[tauri::command]
fn open_accessibility(core: State<Arc<Core>>) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        use std::io::Write;
        if let Some(child) = core.child.lock().unwrap().as_mut() {
            if let Some(input) = child.stdin.as_mut() {
                let _ = writeln!(input, "P");
            }
        }
    }
    #[cfg(target_os = "macos")]
    return open_url(
        "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
    );
    #[cfg(not(target_os = "macos"))]
    Ok(())
}
fn open_url(url: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("open").arg(url).spawn();
    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("rundll32.exe")
        .args(["url.dll,FileProtocolHandler", url])
        .spawn();
    result
        .map(|_| ())
        .map_err(|_| "Couldn't open the system browser.".into())
}
const LANGUAGES: &[&str] = &[
    "Spanish",
    "English",
    "French",
    "German",
    "Italian",
    "Portuguese",
    "Dutch",
    "Polish",
    "Ukrainian",
    "Russian",
    "Arabic",
    "Hindi",
    "Chinese",
    "Japanese",
    "Korean",
    "Turkish",
    "Swedish",
    "Danish",
    "Norwegian",
    "Finnish",
    "Czech",
    "Greek",
    "Romanian",
    "Hungarian",
    "Indonesian",
    "Vietnamese",
    "Thai",
    "Hebrew",
];
#[derive(Deserialize)]
struct Capture {
    kind: String,
    #[serde(default)]
    trusted: bool,
    #[serde(default)]
    settled: bool,
    #[serde(default)]
    text: String,
    #[serde(default)]
    app: String,
    #[serde(default)]
    x: f64,
    #[serde(default)]
    y: f64,
}
fn capture(app: tauri::AppHandle, core: Arc<Core>) {
    std::thread::spawn(move || {
        let path = app.path().app_cache_dir().unwrap().join(if cfg!(windows) {
            "claro-capture.exe"
        } else {
            "claro-capture"
        });
        let _ = std::fs::create_dir_all(path.parent().unwrap());
        if std::fs::write(&path, include_bytes!(env!("CLARO_CAPTURE"))).is_err() {
            let _ = app.emit("capture-error", "Couldn't prepare native selection reader.");
            return;
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700));
        }
        let mut command = std::process::Command::new(&path);
        command
            .arg(std::process::id().to_string())
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x08000000);
        }
        let mut child = match command.spawn() {
            Ok(c) => c,
            Err(_) => {
                let _ = app.emit("capture-error", "Couldn't start native selection reader.");
                return;
            }
        };
        let stdout = child.stdout.take().unwrap();
        *core.child.lock().unwrap() = Some(child);
        control(&core);
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            let Ok(event) = serde_json::from_str::<Capture>(&line) else {
                continue;
            };
            if event.kind == "selection" {
                qa(&app, &core, "selection_seen");
            }
            if event.kind == "status" {
                core.permission.store(event.trusted, Ordering::SeqCst);
                if !event.trusted {
                    core.tracker.lock().unwrap().clear();
                    dismiss(&app, &core, false);
                }
                let _ = app.emit("state-changed", ());
                continue;
            }
            if event.kind == "dismiss" {
                dismiss(&app, &core, true);
                continue;
            }
            if event.kind == "clear" {
                core.tracker.lock().unwrap().clear();

                dismiss(&app, &core, false);
                continue;
            }
            let settings = core.settings.lock().unwrap().clone();
            if core.paused.load(Ordering::SeqCst)
                || !settings.consent
                || settings
                    .excluded_apps
                    .iter()
                    .any(|s| s.eq_ignore_ascii_case(&event.app))
            {
                dismiss(&app, &core, false);
                continue;
            }
            let generation = {
                let mut tracker = core.tracker.lock().unwrap();
                tracker.select(&event.app, &event.text).or_else(|| {
                    if event.settled {
                        tracker.settle(&event.app, &event.text)
                    } else {
                        None
                    }
                })
            };
            let Some(generation) = generation else {
                continue;
            };
            qa(&app, &core, "selection_accepted");
            abort_cloud(&core);
            let is_cloud = settings.mode == "cloud";
            let owner = core.clone();
            let app = app.clone();
            let core = core.clone();
            let task = tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(if event.settled { 30 } else { 150 }))
                    .await;
                if !core.tracker.lock().unwrap().is_current(generation) {
                    return;
                }
                core.tracker.lock().unwrap().started(generation);
                if !show_popup(&app, &core, event.x, event.y) {
                    return;
                }
                if !emit_current(
                    &app,
                    &core,
                    generation,
                    json!({"state":"loading","target":settings.target,"mode":settings.mode}),
                ) {
                    return;
                }
                qa(&app, &core, "translation_started");
                let result = if settings.mode == "offline" {
                    offline::translate_auto(&app, &settings, &event.text, core.clone(), generation)
                        .await
                } else {
                    match key_for(&settings) {
                        Ok(k) => {
                            provider::translate(&core.client, &settings, &k, &event.text).await
                        }
                        Err(e) => Err(e),
                    }
                };
                if !core.tracker.lock().unwrap().is_current(generation) {
                    return;
                }
                qa(&app, &core, "translation_completed");
                let payload = match result {
                    Ok(text) => {
                        json!({"state":"ready","text":text,"target":settings.target,"mode":settings.mode})
                    }
                    Err(error) => {
                        json!({"state":"error","error":error,"target":settings.target,"mode":settings.mode})
                    }
                };
                emit_current(&app, &core, generation, payload);
            });
            if is_cloud {
                *owner.cloud_task.lock().unwrap() = Some(task);
            }
        }
        if let Some(mut child) = core.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    });
}
fn ensure_popup(app: &tauri::AppHandle, core: &Arc<Core>) -> Result<(), String> {
    if app.get_webview_window("popup").is_some() {
        return Ok(());
    }
    core.popup_ready.store(false, Ordering::SeqCst);
    qa(app, core, "popup_creation");
    let result = tauri::WebviewWindowBuilder::new(
        app,
        "popup",
        tauri::WebviewUrl::App("index.html?popup".into()),
    )
    .title("Claro translation")
    .inner_size(336.0, 180.0)
    .visible(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .resizable(false)
    .focused(false)
    .focusable(false)
    .shadow(false)
    .build();
    let w = result
        .map_err(|_| "Couldn't open the translation panel. Reopen Claro settings and try again.")?;
    {
        #[cfg(target_os = "macos")]
        {
            let window = w.clone();
            let _ = w.run_on_main_thread(move || unsafe {
                use objc::{msg_send, sel, sel_impl};
                if let Ok(pointer) = window.ns_window() {
                    let ns = pointer as *mut objc::runtime::Object;
                    let mask: u64 = msg_send![ns, styleMask];
                    let _: () = msg_send![ns,setStyleMask:mask|128u64];
                    let _: () = msg_send![ns,setHidesOnDeactivate:false];
                }
            });
        }
    }
    Ok(())
}
fn emit_current(app: &tauri::AppHandle, core: &Arc<Core>, generation: u64, payload: Value) -> bool {
    let tracker = core.tracker.lock().unwrap();
    if !tracker.is_current(generation) || core.paused.load(Ordering::SeqCst) {
        return false;
    }
    emit_popup(app, core, payload);
    true
}
fn emit_popup(app: &tauri::AppHandle, core: &Arc<Core>, payload: Value) {
    *core.pending_popup.lock().unwrap() = Some(payload.clone());
    if core.popup_ready.load(Ordering::SeqCst) {
        let _ = app.emit_to("popup", "translation", payload);
        if let Some(w) = app.get_webview_window("popup") {
            let _ = w.show();
            qa(app, core, "popup_show");
        }
    }
}
#[tauri::command]
fn popup_ready(app: tauri::AppHandle, core: State<Arc<Core>>) {
    core.popup_ready.store(true, Ordering::SeqCst);
    qa(&app, &core, "renderer_ready");
    let payload = core.pending_popup.lock().unwrap().clone();
    if let Some(payload) = payload {
        emit_popup(&app, &core, payload);
    }
}
#[tauri::command]
fn set_launch_at_login(app: tauri::AppHandle, enabled: bool) -> Result<(), String> {
    let result = if enabled {
        app.autolaunch().enable()
    } else {
        app.autolaunch().disable()
    };
    result.map_err(|_| {
        "Couldn't update launch at login. Keep Claro in its installed location and try again."
            .into()
    })
}
fn show_popup(app: &tauri::AppHandle, core: &Arc<Core>, x: f64, y: f64) -> bool {
    core.popup_epoch.fetch_add(1, Ordering::SeqCst);
    if let Err(error) = ensure_popup(app, core) {
        *core.status_error.lock().unwrap() = Some(error.clone());
        let _ = app.emit("capture-error", error);
        return false;
    }
    *core.status_error.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window("popup") {
        let mut px = x;
        let mut py = y + 10.0;
        if let Ok(monitors) = w.available_monitors() {
            for m in monitors {
                let scale = m.scale_factor();
                let p = m.position();
                let size = m.size();
                #[cfg(windows)]
                {
                    let left = p.x as f64;
                    let top = p.y as f64;
                    let right = left + size.width as f64;
                    let bottom = top + size.height as f64;
                    if x >= left && x <= right && y >= top && y <= bottom {
                        let px = x.clamp(
                            left + 12.0 * scale,
                            (right - 348.0 * scale).max(left + 12.0 * scale),
                        );
                        let py = if y + 290.0 * scale > bottom {
                            (y - 280.0 * scale).max(top + 12.0 * scale)
                        } else {
                            y + 10.0 * scale
                        };
                        let _ = w.set_position(tauri::PhysicalPosition::new(px as i32, py as i32));
                        return true;
                    }
                }
                let left = p.x as f64 / scale;
                let top = p.y as f64 / scale;
                let right = left + size.width as f64 / scale;
                let bottom = top + size.height as f64 / scale;
                if x >= left && x <= right && y >= top && y <= bottom {
                    px = x.clamp(left + 12.0, right - 348.0);
                    py = if py + 250.0 > bottom {
                        (y - 240.0).max(top + 12.0)
                    } else {
                        py
                    };
                    break;
                }
            }
        }
        let _ = w.set_position(tauri::LogicalPosition::new(px, py));
        return true;
    }
    false
}
fn settings(app: &tauri::AppHandle, manual: bool) {
    if app.get_webview_window("settings").is_none() {
        let builder = tauri::WebviewWindowBuilder::new(
            app,
            "settings",
            tauri::WebviewUrl::App(if manual {
                "index.html?manual".into()
            } else {
                "index.html".into()
            }),
        )
        .title("Claro")
        .inner_size(650.0, 600.0)
        .min_inner_size(650.0, 600.0)
        .max_inner_size(650.0, 600.0)
        .resizable(false)
        .center();
        #[cfg(target_os = "macos")]
        let builder = builder
            .title_bar_style(tauri::TitleBarStyle::Overlay)
            .hidden_title(true);
        let _ = builder.build();
    }
    if let Some(w) = app.get_webview_window("settings") {
        let _ = w.show();
        let _ = w.set_focus();
        if manual {
            let _ = app.emit_to("settings", "show-manual", ());
        }
    }
}
#[tauri::command]
fn copy_translation(text: String) -> Result<(), String> {
    if text.len() > 30000 {
        return Err("Text is too long to copy.".into());
    }
    arboard::Clipboard::new()
        .and_then(|mut clipboard| clipboard.set_text(text))
        .map_err(|_| "Couldn't copy the translation.".into())
}
#[tauri::command]
fn popup_rendered(app: tauri::AppHandle, core: State<Arc<Core>>, state: String) {
    if ["loading", "ready", "error"].contains(&state.as_str()) {
        qa(&app, &core, &format!("rendered_{}", state));
    }
}
#[tauri::command]
fn resize_popup(app: tauri::AppHandle, height: f64) {
    if let Some(w) = app.get_webview_window("popup") {
        let _ = w.set_size(tauri::LogicalSize::new(336.0, height.clamp(118.0, 280.0)));
    }
}
fn main() {
    let initial = Settings::default();
    let core = Arc::new(Core {
        settings: Mutex::new(initial),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        tracker: Mutex::new(selection::SelectionTracker::default()),
        paused: AtomicBool::new(false),
        permission: AtomicBool::new(!cfg!(target_os = "macos")),
        child: Mutex::new(None),
        popup_ready: AtomicBool::new(false),
        pending_popup: Mutex::new(None),
        popup_epoch: AtomicU64::new(0),
        cloud_task: Mutex::new(None),
        status_error: Mutex::new(None),
        diagnostics: Mutex::new(json!({})),
    });
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app,_,_|settings(app,false)))
        .plugin(tauri_plugin_autostart::Builder::new().arg("--background").build())
        .manage(core.clone())
        .invoke_handler(tauri::generate_handler![
            get_state,
            save_settings,
            remove_key,
            set_paused,
            dismiss_popup,
            translate_manual,
            activate_license,
            download_model,
            open_purchase,
            open_accessibility,copy_translation,resize_popup,popup_ready,set_launch_at_login,popup_rendered
        ])
        .setup(move |app| {
            let path = config_path(app.handle());
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(s) = serde_json::from_slice::<Settings>(&bytes) {
                    *core.settings.lock().unwrap() = s;
                }
            }
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let pause = MenuItem::with_id(app, "pause", "Pause / resume", true, None::<&str>)?;
            let settings_item =
                MenuItem::with_id(app, "settings", "Settings…", true, None::<&str>)?;
            let manual =
                MenuItem::with_id(app, "manual", "Translate manually…", true, None::<&str>)?;
            let quit = MenuItem::with_id(app, "quit", "Quit Claro", true, None::<&str>)?;
            let menu = Menu::with_items(app, &[&pause, &manual, &settings_item, &quit])?;
            let mut icon = vec![0; 32 * 32 * 4];
            for y in 4..28 {
                for x in 4..28 {
                    if (x as i32 - 16).pow(2) + (y as i32 - 16).pow(2) < 140 {
                        let i = (y * 32 + x) * 4;
                        icon[i] = 29;
                        icon[i + 1] = 40;
                        icon[i + 2] = 48;
                        icon[i + 3] = 255;
                    }
                }
            }
            TrayIconBuilder::new()
                .icon(tauri::image::Image::new_owned(icon, 32, 32))
                .icon_as_template(true)
                .tooltip("Claro · select to translate")
                .menu(&menu)
                .on_menu_event(|app, event| {
                    let core = app.state::<Arc<Core>>();
                    match event.id.as_ref() {
                        "quit" => {
                            if let Some(mut c) = core.child.lock().unwrap().take() {
                                let _ = c.kill();
                                let _ = c.wait();
                            }
                            app.exit(0)
                        }
                        "settings" => {
                            dismiss(app, &core, false);
                            settings(app,false)
                        }
                        "manual" => {
                            settings(app,true);
                        }
                        "pause" => {
                            let next = !core.paused.load(Ordering::SeqCst);
                            core.paused.store(next, Ordering::SeqCst);
                            core.tracker.lock().unwrap().clear();
                            control(&core);
                            dismiss(app, &core, false);
                            let _ = app.emit("state-changed", ());
                        }
                        _ => (),
                    }
                })
                .build(app)?;
            let handle = app.handle().clone();
            let refresh_core=core.clone();let refresh_app=handle.clone();tauri::async_runtime::spawn(async move { license::refresh(&refresh_core.client).await; offline::configure(refresh_app.clone(),refresh_core.settings.lock().unwrap().clone()); let _=refresh_app.emit("state-changed",()); });
            offline::configure(handle.clone(),core.settings.lock().unwrap().clone());
            capture(handle.clone(), core.clone());
            if !std::env::args().any(|a|a=="--background"){settings(&handle,false);}
            #[cfg(debug_assertions)]if std::env::args().any(|a|a=="--preview-popup"){let preview_app=handle.clone();let preview_core=core.clone();tauri::async_runtime::spawn(async move {tokio::time::sleep(Duration::from_millis(1600)).await;show_popup(&preview_app,&preview_core,660.0,300.0);emit_popup(&preview_app,&preview_core,json!({"state":"ready","text":"Hay un mundo entero esperando a ser entendido.","target":"Spanish","mode":"offline"}));});}
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("Couldn't start Claro")
        .run(|app, event| match event {
            tauri::RunEvent::ExitRequested { code: None, api, .. } => api.prevent_exit(),
            #[cfg(target_os = "macos")]
            tauri::RunEvent::Reopen { has_visible_windows:false,.. } => settings(app, false),
            _ => (),
        });
}

#[cfg(test)]
mod test_http {
    pub fn server(status: u16, body: &str) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let body = body.to_owned();
        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(std::time::Duration::from_secs(3)))
                .unwrap();
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            loop {
                let n = stream.read(&mut buf).unwrap();
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                if let Some(i) = data.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&data[..i]).to_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|l| {
                            l.strip_prefix("content-length: ")
                                .and_then(|n| n.trim().parse::<usize>().ok())
                        })
                        .unwrap_or(0);
                    if data.len() >= i + 4 + length {
                        break;
                    }
                }
            }
            let _ = tx.send(String::from_utf8_lossy(&data).to_string());
            write!(stream,"HTTP/1.1 {} OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",status,body.len(),body).unwrap();
        });
        (url, rx)
    }
}
