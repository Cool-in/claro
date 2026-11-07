#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
mod cache;
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

#[derive(Clone)]
struct PendingPopup {
    generation: u64,
    payload: Value,
}
#[derive(Clone, Copy)]
struct PopupDrag {
    window: (i32, i32),
    cursor: (f64, f64),
    scale: f64,
}
struct Core {
    settings: Mutex<Settings>,
    client: reqwest::Client,
    tracker: Mutex<selection::SelectionTracker>,
    paused: AtomicBool,
    permission: AtomicBool,
    child: Mutex<Option<std::process::Child>>,
    popup_ready: AtomicBool,
    popup_picker: AtomicBool,
    popup_saved_position: Mutex<Option<(i32, i32)>>,
    popup_dragging: AtomicBool,
    popup_drag: Mutex<Option<PopupDrag>>,
    settings_drag: Mutex<Option<PopupDrag>>,
    popup_move_epoch: AtomicU64,
    pending_popup: Mutex<Option<PendingPopup>>,
    popup_epoch: AtomicU64,
    cloud_task: Mutex<Option<tauri::async_runtime::JoinHandle<()>>>,
    status_error: Mutex<Option<String>>,
    cache: cache::TranslationCache,
    #[cfg(debug_assertions)]
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
    if event == "selection_accepted" || event == "target_changed" {
        state["request_started_ms"] = json!(now);
        state["selection_to_ready_ms"] = Value::Null;
        state["translation_duration_ms"] = Value::Null;
        state["offline_queue_wait_ms"] = Value::Null;
        state["offline_inference_ms"] = Value::Null;
    }
    if event == "translation_completed" {
        if let Some(start) = state["translation_started_at_ms"].as_u64() {
            state["translation_duration_ms"] = json!(now.saturating_sub(start));
        }
    }
    if event == "offline_inference_started" {
        if let Some(start) = state["offline_queue_entered_at_ms"].as_u64() {
            state["offline_queue_wait_ms"] = json!(now.saturating_sub(start));
        }
    }
    if event == "offline_inference_completed" {
        if let Some(start) = state["offline_inference_started_at_ms"].as_u64() {
            state["offline_inference_ms"] = json!(now.saturating_sub(start));
        }
    }
    if event == "rendered_ready" && state["selection_to_ready_ms"].is_null() {
        if let Some(start) = state["request_started_ms"].as_u64() {
            state["selection_to_ready_ms"] = json!(now.saturating_sub(start));
        }
    }
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
    cached_key(&account, || {
        secret(&account)?
            .get_password()
            .map_err(|_| "Add your provider API key in Settings.".into())
    })
}
// The Keychain is slow and can prompt, so the active provider's key is kept in
// memory after the first read. It is cleared whenever a key is saved or removed.
static KEY_CACHE: Mutex<Option<(String, String)>> = Mutex::new(None);
fn cached_key(
    account: &str,
    load: impl FnOnce() -> Result<String, String>,
) -> Result<String, String> {
    if let Some((cached_account, key)) = KEY_CACHE.lock().unwrap().as_ref() {
        if cached_account == account {
            return Ok(key.clone());
        }
    }
    let key = load()?;
    *KEY_CACHE.lock().unwrap() = Some((account.to_string(), key.clone()));
    Ok(key)
}
fn forget_cached_key() {
    *KEY_CACHE.lock().unwrap() = None;
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
    json!({"settings":settings,"hasKey":has_key,"licensed":licensed(),"paused":core.paused.load(Ordering::SeqCst),"permission":core.permission.load(Ordering::SeqCst),"checkoutAvailable":!r.checkout_url.is_empty()&&r.model.is_some(),"modelAvailable":r.model.is_some(),"priceLabel":r.price_label,"launchAtLogin":app.autolaunch().is_enabled().unwrap_or(false),"statusError":core.status_error.lock().unwrap().clone()})
}
#[tauri::command]
fn save_settings(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    settings: Settings,
    api_key: Option<String>,
) -> Result<(), String> {
    provider::endpoint(&settings.base_url)?;
    if !["system", "light", "dark"].contains(&settings.popup_theme.as_str()) {
        return Err("Choose a valid popup theme.".into());
    }
    if settings.model.trim().is_empty() || settings.model.len() > 200 {
        return Err("Enter a provider model name.".into());
    }
    if !["Neutral", "Formal", "Casual"].contains(&settings.tone.as_str()) {
        return Err("Choose a supported tone.".into());
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
            forget_cached_key();
        }
    }
    persist_settings(&app, &settings)?;
    *core.settings.lock().unwrap() = settings.clone();
    offline::configure(app.clone(), settings);
    warm_provider(&core);
    control(&core);
    core.tracker.lock().unwrap().clear();
    dismiss(&app, &core, false);
    if core.settings.lock().unwrap().consent {
        let _ = ensure_popup(&app, &core);
    }
    Ok(())
}
fn persist_settings(app: &tauri::AppHandle, settings: &Settings) -> Result<(), String> {
    let path = config_path(app);
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|_| "Couldn't create settings folder.")?;
    let temp = path.with_extension("tmp");
    std::fs::write(&temp, serde_json::to_vec(settings).unwrap())
        .map_err(|_| "Couldn't save settings.")?;
    std::fs::rename(temp, path).map_err(|_| "Couldn't finish saving settings.")?;
    Ok(())
}
#[tauri::command]
fn set_popup_picker(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    open: bool,
) -> Result<(), String> {
    if open && !core.tracker.lock().unwrap().has_active() {
        return Err("Select some text again to choose its language.".into());
    }
    core.popup_picker.store(open, Ordering::SeqCst);
    qa(
        &app,
        &core,
        if open {
            "picker_opened"
        } else {
            "picker_closed"
        },
    );
    Ok(())
}

fn placement_path(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path()
        .app_config_dir()
        .unwrap()
        .join("popup-position.json")
}
fn save_placement(app: &tauri::AppHandle, position: (i32, i32)) {
    let path = placement_path(app);
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(&position) {
        let temp = path.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(temp, path);
        }
    }
}
#[tauri::command]
fn start_popup_drag(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    let window = app
        .get_webview_window("popup")
        .ok_or("The translation panel is closed.")?;
    let position = window
        .outer_position()
        .map_err(|_| "Couldn't read the popup position.")?;
    *core.popup_drag.lock().unwrap() = Some(PopupDrag {
        window: (position.x, position.y),
        cursor: (x, y),
        scale: window.scale_factor().unwrap_or(1.0),
    });
    core.popup_dragging.store(true, Ordering::SeqCst);
    qa(&app, &core, "drag_started");
    Ok(())
}
fn move_drag(app: &tauri::AppHandle, core: &Arc<Core>, x: f64, y: f64) -> Result<(), String> {
    let drag = { *core.popup_drag.lock().unwrap() };
    let Some(drag) = drag else { return Ok(()) };
    if let Some(window) = app.get_webview_window("popup") {
        let position = (
            drag.window.0 + ((x - drag.cursor.0) * drag.scale).round() as i32,
            drag.window.1 + ((y - drag.cursor.1) * drag.scale).round() as i32,
        );
        window
            .set_position(tauri::PhysicalPosition::new(position.0, position.1))
            .map_err(|_| "Couldn't move the popup.")?;
        *core.popup_saved_position.lock().unwrap() = Some(position);
    }
    Ok(())
}
#[tauri::command]
fn move_popup_drag(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    move_drag(&app, &core, x, y)
}
fn finish_drag(app: &tauri::AppHandle, core: &Arc<Core>) {
    core.popup_drag.lock().unwrap().take();
    core.popup_dragging.store(false, Ordering::SeqCst);
    core.popup_move_epoch.fetch_add(1, Ordering::SeqCst);
    let position = { *core.popup_saved_position.lock().unwrap() };
    if let Some(position) = position {
        save_placement(app, position);
    }
    qa(app, core, "drag_finished");
}
#[tauri::command]
fn end_popup_drag(app: tauri::AppHandle, core: State<Arc<Core>>) {
    finish_drag(&app, &core);
}
// Settings uses its own drag session; moving it cannot touch popup placement.
#[tauri::command]
fn start_settings_drag(
    window: tauri::WebviewWindow,
    core: State<Arc<Core>>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("Open Claro settings to move this window.".into());
    }
    let position = window
        .outer_position()
        .map_err(|_| "Couldn't read the Settings position.")?;
    *core.settings_drag.lock().unwrap() = Some(PopupDrag {
        window: (position.x, position.y),
        cursor: (x, y),
        scale: window.scale_factor().unwrap_or(1.0),
    });
    Ok(())
}
#[tauri::command]
fn move_settings_drag(
    window: tauri::WebviewWindow,
    core: State<Arc<Core>>,
    x: f64,
    y: f64,
) -> Result<(), String> {
    if window.label() != "settings" {
        return Err("Open Claro settings to move this window.".into());
    }
    let drag = { *core.settings_drag.lock().unwrap() };
    if let Some(drag) = drag {
        window
            .set_position(tauri::PhysicalPosition::new(
                drag.window.0 + ((x - drag.cursor.0) * drag.scale).round() as i32,
                drag.window.1 + ((y - drag.cursor.1) * drag.scale).round() as i32,
            ))
            .map_err(|_| "Couldn't move Settings.")?;
    }
    Ok(())
}
#[tauri::command]
fn end_settings_drag(window: tauri::WebviewWindow, core: State<Arc<Core>>) {
    if window.label() == "settings" {
        core.settings_drag.lock().unwrap().take();
    }
}
#[tauri::command]
fn get_popup_theme(core: State<Arc<Core>>) -> String {
    core.settings.lock().unwrap().popup_theme.clone()
}
#[tauri::command]
fn set_popup_theme(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    theme: String,
) -> Result<(), String> {
    if !["system", "light", "dark"].contains(&theme.as_str()) {
        return Err("Choose System, Light or Dark.".into());
    }
    let mut settings = core.settings.lock().unwrap().clone();
    settings.popup_theme = theme.clone();
    persist_settings(&app, &settings)?;
    *core.settings.lock().unwrap() = settings;
    let _ = app.emit("popup-theme-changed", theme);
    let _ = app.emit_to("settings", "state-changed", ());
    qa(&app, &core, "theme_changed");
    Ok(())
}
#[tauri::command]
fn follow_selection(app: tauri::AppHandle, core: State<Arc<Core>>) -> Result<(), String> {
    core.popup_move_epoch.fetch_add(1, Ordering::SeqCst);
    core.popup_dragging.store(false, Ordering::SeqCst);
    *core.popup_saved_position.lock().unwrap() = None;
    match std::fs::remove_file(placement_path(&app)) {
        Ok(()) => (),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(_) => return Err("Couldn't reset the saved popup position.".into()),
    }
    let anchor = { core.tracker.lock().unwrap().anchor() };
    if let (Some(window), Some((x, y, top))) = (app.get_webview_window("popup"), anchor) {
        let scale = window.scale_factor().unwrap_or(1.0);
        let size = window
            .inner_size()
            .unwrap_or(tauri::PhysicalSize::new(232, 84));
        position_popup(
            &window,
            x,
            y,
            top,
            size.width as f64 / scale,
            size.height as f64 / scale,
        );
    }
    qa(&app, &core, "placement_reset");
    Ok(())
}
fn clamp_popup_position(
    position: (i32, i32),
    size: (i32, i32),
    area: (i32, i32, i32, i32),
) -> (i32, i32) {
    let left = area.0 + 8;
    let top = area.1 + 8;
    (
        position
            .0
            .clamp(left, (area.0 + area.2 - size.0 - 8).max(left)),
        position
            .1
            .clamp(top, (area.1 + area.3 - size.1 - 8).max(top)),
    )
}
fn place_popup(
    core: &Arc<Core>,
    window: &tauri::WebviewWindow,
    x: f64,
    y: f64,
    top: f64,
    width: f64,
    height: f64,
) {
    if core.popup_dragging.load(Ordering::SeqCst) {
        return;
    }
    let saved = { *core.popup_saved_position.lock().unwrap() };
    if let Some((x, y)) = saved {
        // set_size is dispatched asynchronously. Clamp using the requested new
        // content size, not the old short card returned by outer_size.
        let scale = window.scale_factor().unwrap_or(1.0);
        let size = (
            (width * scale).ceil() as i32,
            (height * scale).ceil() as i32,
        );
        let mut result = (x, y);
        if let Ok(monitors) = window.available_monitors() {
            if let Some(monitor) = monitors
                .iter()
                .find(|m| {
                    let p = m.position();
                    let z = m.size();
                    x >= p.x && x < p.x + z.width as i32 && y >= p.y && y < p.y + z.height as i32
                })
                .or_else(|| monitors.first())
            {
                let area = monitor.work_area();
                result = clamp_popup_position(
                    (x, y),
                    size,
                    (
                        area.position.x,
                        area.position.y,
                        area.size.width as i32,
                        area.size.height as i32,
                    ),
                );
            }
        }
        let _ = window.set_position(tauri::PhysicalPosition::new(result.0, result.1));
    } else {
        position_popup(window, x, y, top, width, height);
    }
}
#[tauri::command]
fn change_popup_target(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    target: String,
) -> Result<(), String> {
    if !LANGUAGES.contains(&target.as_str()) {
        return Err("Choose a supported target language.".into());
    }
    if core.paused.load(Ordering::SeqCst) {
        return Err("Resume Claro before translating.".into());
    }
    let mut settings = core.settings.lock().unwrap().clone();
    if !settings.consent {
        return Err("Automatic translation is turned off.".into());
    }
    let (generation, selection) = core
        .tracker
        .lock()
        .unwrap()
        .retarget()
        .ok_or("Select some text again before changing its language.")?;
    if settings
        .excluded_apps
        .iter()
        .any(|app| app.eq_ignore_ascii_case(&selection.app))
    {
        return Err("This app is excluded from automatic translation.".into());
    }
    settings.target = target;
    persist_settings(&app, &settings)?;
    *core.settings.lock().unwrap() = settings.clone();
    let _ = app.emit_to("settings", "state-changed", ());
    qa(&app, &core, "target_changed");
    spawn_translation(
        app,
        core.inner().clone(),
        settings,
        selection,
        generation,
        0,
    );
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
    forget_cached_key();
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
    core.popup_picker.store(false, Ordering::SeqCst);
    core.popup_drag.lock().unwrap().take();
    core.popup_dragging.store(false, Ordering::SeqCst);
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
            && (!core.settings.lock().unwrap().consent || core.paused.load(Ordering::SeqCst))
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
    if let Some(hit) = core.cache.get(&s, &text) {
        return Ok(hit);
    }
    let fresh = provider::translate(&core.client, &s, &key_for(&s)?, &text).await?;
    core.cache.put(&s, &text, &fresh);
    Ok(fresh)
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
    let release = license::release();
    if release.model.is_none() {
        return Err("The offline download is being prepared. Purchasing will be available when it is ready.".into());
    }
    let u = release.checkout_url;
    if u.is_empty() {
        return Err("Checkout is not configured in this development build.".into());
    }
    let url = url::Url::parse(&u).map_err(|_| "Invalid checkout URL.")?;
    if url.scheme() != "https" || !matches!(url.host_str(), Some("polar.sh" | "buy.polar.sh")) {
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
    reason: String,
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
    #[serde(default)]
    top: f64,
}
fn point_in_window(x: f64, y: f64, left: f64, top: f64, width: f64, height: f64) -> bool {
    x >= left && x < left + width && y >= top && y < top + height
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
            if event.kind == "pointer-down" {
                let mut bounds = None;
                let inside = app
                    .get_webview_window("popup")
                    .map(|window| {
                        if !window.is_visible().unwrap_or(false) {
                            return false;
                        }
                        let Ok(position) = window.outer_position() else {
                            return false;
                        };
                        let Ok(size) = window.outer_size() else {
                            return false;
                        };
                        #[cfg(target_os = "macos")]
                        let scale = window.scale_factor().unwrap_or(1.0);
                        #[cfg(not(target_os = "macos"))]
                        let scale = 1.0;
                        bounds = Some((
                            position.x as f64 / scale,
                            position.y as f64 / scale,
                            size.width as f64 / scale,
                            size.height as f64 / scale,
                            scale,
                        ));
                        point_in_window(
                            event.x,
                            event.y,
                            position.x as f64 / scale,
                            position.y as f64 / scale,
                            size.width as f64 / scale,
                            size.height as f64 / scale,
                        )
                    })
                    .unwrap_or(false);
                #[cfg(debug_assertions)]
                {
                    core.diagnostics.lock().unwrap()["last_pointer"] = json!({"point":[event.x,event.y],"popup_bounds_and_scale":bounds,"inside":inside});
                }
                qa(
                    &app,
                    &core,
                    if inside {
                        "pointer_inside"
                    } else {
                        "pointer_outside"
                    },
                );
                if !inside {
                    dismiss(&app, &core, true);
                }
                continue;
            }
            if event.kind == "pointer-up" {
                if core.popup_dragging.load(Ordering::SeqCst) {
                    let app = app.clone();
                    let core = core.clone();
                    tauri::async_runtime::spawn(async move {
                        tokio::time::sleep(Duration::from_millis(500)).await;
                        if core.popup_dragging.load(Ordering::SeqCst) {
                            finish_drag(&app, &core);
                        }
                    });
                }
                continue;
            }
            if event.kind == "dismiss" {
                dismiss(&app, &core, true);
                continue;
            }
            if event.kind == "clear" {
                if event.reason == "self" && core.pending_popup.lock().unwrap().is_some() {
                    continue;
                }
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
            core.tracker
                .lock()
                .unwrap()
                .set_anchor(generation, event.x, event.y);
            core.tracker.lock().unwrap().set_top(generation, event.top);
            let delay = if event.settled { 30 } else { 150 };
            let selection = selection::ActiveSelection {
                app: event.app,
                text: event.text,
                x: event.x,
                y: event.y,
                top: event.top,
            };
            spawn_translation(
                app.clone(),
                core.clone(),
                settings,
                selection,
                generation,
                delay,
            );
        }
        if let Some(mut child) = core.child.lock().unwrap().take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    });
}
fn warm_provider(core: &Arc<Core>) {
    let s = core.settings.lock().unwrap().clone();
    if s.mode != "cloud" || !s.consent || key_for(&s).is_err() {
        return;
    }
    let client = core.client.clone();
    tauri::async_runtime::spawn(async move {
        provider::warm(&client, &s).await;
    });
}
fn spawn_translation(
    app: tauri::AppHandle,
    core: Arc<Core>,
    settings: Settings,
    selection: selection::ActiveSelection,
    generation: u64,
    delay_ms: u64,
) {
    abort_cloud(&core);
    let is_cloud = settings.mode == "cloud";
    let owner = core.clone();
    let task = tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(delay_ms)).await;
        if !core.tracker.lock().unwrap().is_current(generation) {
            return;
        }
        core.tracker.lock().unwrap().started(generation);
        if !show_popup(&app, &core, selection.x, selection.y, selection.top) {
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
            offline::translate_auto(&app, &settings, &selection.text, core.clone(), generation)
                .await
        } else if let Some(hit) = core.cache.get(&settings, &selection.text) {
            Ok(hit)
        } else {
            match key_for(&settings) {
                Ok(k) => {
                    let mut last: Option<std::time::Instant> = None;
                    let (stream_app, stream_core) = (app.clone(), core.clone());
                    let (target, mode) = (settings.target.clone(), settings.mode.clone());
                    let mut on_partial = move |partial: &str| {
                        if last.map_or(false, |t| t.elapsed() < Duration::from_millis(60)) {
                            return;
                        }
                        last = Some(std::time::Instant::now());
                        emit_current(
                            &stream_app,
                            &stream_core,
                            generation,
                            json!({"state":"streaming","text":partial,"target":target,"mode":mode}),
                        );
                    };
                    let fresh = provider::translate_stream(
                        &core.client,
                        &settings,
                        &k,
                        &selection.text,
                        &mut on_partial,
                    )
                    .await;
                    if let Ok(text) = &fresh {
                        core.cache.put(&settings, &selection.text, text);
                    }
                    fresh
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
    .inner_size(232.0, 84.0)
    .visible(false)
    .decorations(false)
    .transparent(true)
    .background_color(tauri::webview::Color(0, 0, 0, 0))
    .accept_first_mouse(true)
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
                    // This is TaoWindow (NSWindow), not NSPanel. Its canBecomeKeyWindow
                    // override already prevents activation. A panel-only style bit here
                    // can interfere with ordinary mouse delivery.
                    let _: () = msg_send![ns,setMovableByWindowBackground:false];
                    let _: () = msg_send![ns,setIgnoresMouseEvents:false];
                    let _: () = msg_send![ns,setHidesOnDeactivate:false];
                }
            });
        }
    }
    Ok(())
}
fn valid_popup(tracker: &selection::SelectionTracker, paused: bool, generation: u64) -> bool {
    tracker.is_current(generation) && !paused
}
fn deliver_popup(app: &tauri::AppHandle, core: &Arc<Core>, generation: u64) {
    let handle = app.clone();
    let core = core.clone();
    let _ = app.run_on_main_thread(move || {
        // Never dispatch UI while holding a mutex. run_on_main_thread runs inline
        // when called by a command, so nested tracker locks would deadlock.
        let payload = {
            let tracker = core.tracker.lock().unwrap();
            if !valid_popup(&tracker, core.paused.load(Ordering::SeqCst), generation) {
                return;
            }
            let pending = core.pending_popup.lock().unwrap();
            match pending.as_ref() {
                Some(value) if value.generation == generation => value.payload.clone(),
                _ => return,
            }
        };
        let _ = handle.emit_to("popup", "translation", payload);
        if let Some(window) = handle.get_webview_window("popup") {
            #[cfg(target_os = "macos")]
            unsafe {
                use objc::{msg_send, sel, sel_impl};
                if let Ok(pointer) = window.ns_window() {
                    let ns = pointer as *mut objc::runtime::Object;
                    let _: () = msg_send![ns, orderFrontRegardless];
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = window.show();
            qa(&handle, &core, "popup_show");
        }
    });
}
fn emit_current(
    app: &tauri::AppHandle,
    core: &Arc<Core>,
    generation: u64,
    mut payload: Value,
) -> bool {
    payload["generation"] = json!(generation);
    {
        let tracker = core.tracker.lock().unwrap();
        if !valid_popup(&tracker, core.paused.load(Ordering::SeqCst), generation) {
            return false;
        }
        *core.pending_popup.lock().unwrap() = Some(PendingPopup {
            generation,
            payload,
        });
    }
    if core.popup_ready.load(Ordering::SeqCst) {
        deliver_popup(app, core, generation);
    }
    true
}
#[tauri::command]
fn popup_ready(app: tauri::AppHandle, core: State<Arc<Core>>) {
    core.popup_ready.store(true, Ordering::SeqCst);
    qa(&app, &core, "renderer_ready");
    let generation = {
        core.pending_popup
            .lock()
            .unwrap()
            .as_ref()
            .map(|value| value.generation)
    };
    if let Some(generation) = generation {
        deliver_popup(&app, &core, generation);
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
fn show_popup(app: &tauri::AppHandle, core: &Arc<Core>, x: f64, y: f64, top: f64) -> bool {
    core.popup_epoch.fetch_add(1, Ordering::SeqCst);
    if let Err(error) = ensure_popup(app, core) {
        *core.status_error.lock().unwrap() = Some(error.clone());
        let _ = app.emit("capture-error", error);
        return false;
    }
    *core.status_error.lock().unwrap() = None;
    if let Some(w) = app.get_webview_window("popup") {
        place_popup(core, &w, x, y, top, 232.0, 84.0);
        return true;
    }
    false
}
fn position_popup(
    w: &tauri::WebviewWindow,
    x: f64,
    y: f64,
    selection_top: f64,
    width: f64,
    height: f64,
) {
    let mut px = x;
    let mut py = y + 10.0;
    if let Ok(monitors) = w.available_monitors() {
        for m in monitors {
            let scale = m.scale_factor();
            let p = m.work_area().position;
            let size = m.work_area().size;
            #[cfg(windows)]
            {
                let left = p.x as f64;
                let top = p.y as f64;
                let right = left + size.width as f64;
                let bottom = top + size.height as f64;
                if x >= left && x <= right && y >= top && y <= bottom {
                    let px = x.clamp(
                        left + 12.0 * scale,
                        (right - (width + 12.0) * scale).max(left + 12.0 * scale),
                    );
                    let py = if y + (height + 10.0) * scale > bottom {
                        (selection_top - (height + 10.0) * scale).max(top + 12.0 * scale)
                    } else {
                        y + 10.0 * scale
                    };
                    let _ = w.set_position(tauri::PhysicalPosition::new(px as i32, py as i32));
                    return;
                }
            }
            let left = p.x as f64 / scale;
            let top = p.y as f64 / scale;
            let right = left + size.width as f64 / scale;
            let bottom = top + size.height as f64 / scale;
            if x >= left && x <= right && y >= top && y <= bottom {
                px = x.clamp(left + 12.0, (right - width - 12.0).max(left + 12.0));
                py = if py + height > bottom {
                    (selection_top - height - 10.0).max(top + 12.0)
                } else {
                    py
                };
                break;
            }
        }
    }
    let _ = w.set_position(tauri::LogicalPosition::new(px, py));
}
fn settings(app: &tauri::AppHandle, manual: bool) {
    if let Some(core) = app.try_state::<Arc<Core>>() {
        dismiss(app, &core, false);
    }
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
fn popup_rendered(
    app: tauri::AppHandle,
    core: State<Arc<Core>>,
    state: String,
    generation: Option<u64>,
) {
    if !generation
        .map(|value| core.tracker.lock().unwrap().is_current(value))
        .unwrap_or(false)
    {
        return;
    }
    if ["loading", "ready", "error"].contains(&state.as_str()) {
        qa(&app, &core, &format!("rendered_{}", state));
    }
}
#[tauri::command]
fn resize_popup(app: tauri::AppHandle, core: State<Arc<Core>>, width: f64, height: f64) {
    if let Some(w) = app.get_webview_window("popup") {
        let width = width.clamp(176.0, 336.0);
        let height = height.clamp(64.0, 380.0);
        let _ = w.set_size(tauri::LogicalSize::new(width, height));
        let anchor = { core.tracker.lock().unwrap().anchor() };
        if let Some((x, y, top)) = anchor {
            place_popup(&core, &w, x, y, top, width, height);
        }
    }
}
fn main() {
    let initial = Settings::default();
    let core = Arc::new(Core {
        settings: Mutex::new(initial),
        client: reqwest::Client::builder()
            .timeout(Duration::from_secs(35))
            .tcp_nodelay(true)
            .tcp_keepalive(Duration::from_secs(30))
            .pool_idle_timeout(Duration::from_secs(300))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .unwrap(),
        tracker: Mutex::new(selection::SelectionTracker::default()),
        paused: AtomicBool::new(false),
        permission: AtomicBool::new(!cfg!(target_os = "macos")),
        child: Mutex::new(None),
        popup_ready: AtomicBool::new(false),
        popup_picker: AtomicBool::new(false),
        popup_saved_position: Mutex::new(None),
        popup_dragging: AtomicBool::new(false),
        popup_drag: Mutex::new(None),
        settings_drag: Mutex::new(None),
        popup_move_epoch: AtomicU64::new(0),
        pending_popup: Mutex::new(None),
        popup_epoch: AtomicU64::new(0),
        cloud_task: Mutex::new(None),
        status_error: Mutex::new(None),
        cache: cache::TranslationCache::default(),
        #[cfg(debug_assertions)]
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
            start_settings_drag,move_settings_drag,end_settings_drag,open_accessibility,copy_translation,resize_popup,popup_ready,set_launch_at_login,popup_rendered,change_popup_target,set_popup_picker,start_popup_drag,move_popup_drag,end_popup_drag,get_popup_theme,set_popup_theme,follow_selection
        ])
        .setup(move |app| {
            let path = config_path(app.handle());
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(s) = serde_json::from_slice::<Settings>(&bytes) {
                    *core.settings.lock().unwrap() = s;
                }
            }
            if let Ok(bytes)=std::fs::read(placement_path(app.handle())){if let Ok(position)=serde_json::from_slice::<(i32,i32)>(&bytes){*core.popup_saved_position.lock().unwrap()=Some(position);}}
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
            warm_provider(&core);
            capture(handle.clone(), core.clone());
            if core.settings.lock().unwrap().consent { let _ = ensure_popup(&handle, &core); }
            if !std::env::args().any(|a|a=="--background"){settings(&handle,false);}
            #[cfg(debug_assertions)]if std::env::args().any(|a|a=="--preview-popup"){
              let preview_app=handle.clone();let preview_core=core.clone();
              tauri::async_runtime::spawn(async move{
                tokio::time::sleep(Duration::from_millis(1600)).await;
                let generation={let mut tracker=preview_core.tracker.lock().unwrap();let Some(generation)=tracker.select("claro.qa.fixture","Good morning.")else{return;};tracker.set_anchor(generation,660.0,300.0);tracker.started(generation);generation};
                qa(&preview_app,&preview_core,"preview_seeded");
                if show_popup(&preview_app,&preview_core,660.0,300.0,280.0){emit_current(&preview_app,&preview_core,generation,json!({"state":"ready","text":"Buen día.","target":"Spanish","mode":"offline"}));}
              });
            }
            Ok(())
        })
        .on_window_event(|window,event|{
          let core=window.state::<Arc<Core>>();
          if window.label()=="settings" {
            #[cfg(debug_assertions)]
            if let tauri::WindowEvent::Moved(position)=event {
              core.diagnostics.lock().unwrap()["settings_position"] = json!([position.x,position.y]);
              qa(window.app_handle(),&core,"settings_moved");
            }
            if matches!(event,tauri::WindowEvent::Destroyed) { core.settings_drag.lock().unwrap().take(); }
            return;
          }
          if window.label()!="popup"{return;}
          if let tauri::WindowEvent::Moved(position)=event{
            if !core.popup_dragging.load(Ordering::SeqCst){return;}
            let value=(position.x,position.y);*core.popup_saved_position.lock().unwrap()=Some(value);
            let epoch=core.popup_move_epoch.fetch_add(1,Ordering::SeqCst)+1;
            let core=core.inner().clone();let app=window.app_handle().clone();
            tauri::async_runtime::spawn(async move{tokio::time::sleep(Duration::from_millis(250)).await;if core.popup_move_epoch.load(Ordering::SeqCst)==epoch{save_placement(&app,value);qa(&app,&core,"placement_saved");}});
          }
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
        server_typed(status, "application/json", body)
    }
    pub fn server_typed(
        status: u16,
        content_type: &str,
        body: &str,
    ) -> (String, std::sync::mpsc::Receiver<String>) {
        use std::io::{Read, Write};
        let content_type = content_type.to_owned();
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
            write!(stream,"HTTP/1.1 {} OK\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",status,content_type,body.len(),body).unwrap();
        });
        (url, rx)
    }
}

#[cfg(test)]
mod key_cache_tests {
    use super::*;
    #[test]
    fn key_is_read_once_per_provider_and_cleared_on_change() {
        forget_cached_key();
        let reads = std::cell::Cell::new(0);
        let load = |value: &'static str| {
            let reads = &reads;
            move || {
                reads.set(reads.get() + 1);
                Ok::<String, String>(value.to_string())
            }
        };
        assert_eq!(cached_key("a", load("one")).unwrap(), "one");
        assert_eq!(cached_key("a", load("two")).unwrap(), "one");
        assert_eq!(reads.get(), 1);
        assert_eq!(cached_key("b", load("three")).unwrap(), "three");
        forget_cached_key();
        assert_eq!(cached_key("b", load("four")).unwrap(), "four");
        assert_eq!(reads.get(), 3);
        forget_cached_key();
        assert!(cached_key("c", || Err("missing".into())).is_err());
    }
}
#[cfg(test)]
mod popup_race_tests {
    use super::*;
    #[test]
    fn renderer_ready_cannot_restore_payload_after_clear_or_pause() {
        let mut tracker = selection::SelectionTracker::default();
        let generation = tracker.select("fixture", "synthetic text").unwrap();
        assert!(valid_popup(&tracker, false, generation));
        assert!(!valid_popup(&tracker, true, generation));
        tracker.clear();
        assert!(!valid_popup(&tracker, false, generation));
        let next = tracker.select("fixture", "different text").unwrap();
        assert!(!valid_popup(&tracker, false, generation));
        assert!(valid_popup(&tracker, false, next));
    }
}

#[cfg(test)]
mod pointer_bounds_tests {
    use super::{clamp_popup_position, point_in_window};
    #[test]
    fn expanded_menu_fits_work_area_without_replacing_saved_anchor() {
        let saved = (1168, 1367);
        let area = (0, 30, 1920, 1410);
        let short = clamp_popup_position(saved, (188, 85), area);
        let expanded = clamp_popup_position(saved, (240, 380), area);
        assert_eq!(short, (1168, 1347));
        assert_eq!(expanded, (1168, 1052));
        assert_eq!(clamp_popup_position(saved, (188, 85), area), short);
        assert_eq!(saved, (1168, 1367));
        assert_eq!(
            clamp_popup_position((-300, 0), (240, 380), (-500, 20, 400, 700)),
            (-348, 28)
        );
    }
    #[test]
    fn popup_buttons_and_border_are_inside_actual_window_bounds() {
        assert!(point_in_window(100., 200., 100., 200., 188., 85.));
        assert!(point_in_window(280., 220., 100., 200., 188., 85.));
        assert!(!point_in_window(99., 220., 100., 200., 188., 85.));
        assert!(!point_in_window(288., 220., 100., 200., 188., 85.));
        assert!(!point_in_window(120., 285., 100., 200., 188., 85.));
        assert!(point_in_window(-300., 30., -400., 20., 188., 85.));
    }
}
