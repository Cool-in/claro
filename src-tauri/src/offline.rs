use crate::{license, licensed, provider::Settings};
use futures_util::StreamExt;
use sha2::{Digest, Sha256};
use tauri::{Emitter, Manager};
use tokio::io::AsyncWriteExt;
fn model_dir(app: &tauri::AppHandle) -> std::path::PathBuf {
    app.path().app_data_dir().unwrap().join("models")
}
static DOWNLOADING: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
struct DownloadGuard;
impl Drop for DownloadGuard {
    fn drop(&mut self) {
        DOWNLOADING.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}
fn safe_segment(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 100
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_' || c == '.')
        && !name.starts_with('.')
}
fn validate_manifest(model: &license::Model) -> Result<(), String> {
    if !safe_segment(&model.id)
        || model.files.is_empty()
        || model.bytes == 0
        || model.files.iter().any(|f| {
            !safe_segment(&f.name)
                || f.sha256.len() != 64
                || !f.sha256.chars().all(|c| c.is_ascii_hexdigit())
                || f.bytes == 0
        })
        || model.files.iter().map(|f| f.bytes).sum::<u64>() != model.bytes
    {
        return Err("The release model package is invalid.".into());
    }
    let names: std::collections::HashSet<_> = model.files.iter().map(|f| &f.name).collect();
    if names.len() != model.files.len() {
        return Err("The model package has duplicate files.".into());
    }
    Ok(())
}
pub async fn download(app: &tauri::AppHandle, _client: &reqwest::Client) -> Result<(), String> {
    if DOWNLOADING
        .compare_exchange(
            false,
            true,
            std::sync::atomic::Ordering::SeqCst,
            std::sync::atomic::Ordering::SeqCst,
        )
        .is_err()
    {
        return Err("A model download is already running.".into());
    }
    let _guard = DownloadGuard;
    let model = license::release()
        .model
        .ok_or("The release model package is not configured yet.")?;
    validate_manifest(&model)?;
    let root = model_dir(app);
    std::fs::create_dir_all(&root).map_err(|_| "Couldn't create the model folder.")?;
    let free = fs2::available_space(&root).map_err(|_| "Couldn't check disk space.")?;
    if free < model.bytes + 1024 * 1024 * 1024 {
        return Err("There isn't enough disk space for this model. Keep at least 1 GB free after installation.".into());
    }
    let target = root.join(&model.id);
    if target.exists() {
        return Err("This model is already installed.".into());
    }
    let staging = root.join(format!("{}.partial", model.id));
    std::fs::create_dir_all(&staging).map_err(|_| "Couldn't prepare the download.")?;
    // This client carries no authorization. Model hosts may redirect to HTTPS CDNs.
    let client = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::custom(|attempt| {
            if attempt.previous().len() >= 5 {
                attempt.error("Too many redirects")
            } else if attempt.url().scheme() != "https" {
                attempt.error("HTTPS required")
            } else {
                attempt.follow()
            }
        }))
        .build()
        .map_err(|_| "Couldn't prepare the download connection.")?;
    let mut total = 0u64;
    for file in &model.files {
        let url = url::Url::parse(&file.url).map_err(|_| "Invalid model URL.")?;
        if url.scheme() != "https" {
            return Err("Model downloads require HTTPS.".into());
        }
        let path = staging.join(&file.name);
        let existing = std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
        let mut hash = Sha256::new();
        let mut bytes = 0u64;
        if existing > 0 && existing <= file.bytes {
            use std::io::Read;
            let mut input =
                std::fs::File::open(&path).map_err(|_| "Couldn't read an interrupted download.")?;
            let mut buffer = [0u8; 65536];
            loop {
                let n = input
                    .read(&mut buffer)
                    .map_err(|_| "Couldn't check an interrupted download.")?;
                if n == 0 {
                    break;
                }
                hash.update(&buffer[..n]);
                bytes += n as u64;
            }
            if bytes == file.bytes
                && hex::encode(hash.clone().finalize()) == file.sha256.to_lowercase()
            {
                total += bytes;
                continue;
            }
            if bytes == file.bytes {
                bytes = 0;
                hash = Sha256::new();
            }
        }
        let mut request = client.get(url);
        if bytes > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={}-", bytes));
        }
        let response = request
            .send()
            .await
            .map_err(|_| "Model download interrupted. Retry to resume it.")?;
        if !response.status().is_success() {
            return Err("The model download server returned an error. Retry to resume it.".into());
        }
        if bytes > 0 && response.status() == reqwest::StatusCode::PARTIAL_CONTENT {
            let expected = format!("bytes {}-", bytes);
            let valid = response
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|h| h.to_str().ok())
                .map(|v| v.starts_with(&expected))
                .unwrap_or(false);
            if !valid {
                return Err("The download server returned an invalid resume range.".into());
            }
        } else {
            bytes = 0;
            hash = Sha256::new();
        }
        let mut out = tokio::fs::OpenOptions::new()
            .create(true)
            .write(true)
            .append(bytes > 0)
            .truncate(bytes == 0)
            .open(&path)
            .await
            .map_err(|_| "Couldn't write the downloaded model.")?;
        total += bytes;
        let mut stream = response.bytes_stream();
        while let Some(chunk) =
            tokio::time::timeout(std::time::Duration::from_secs(45), stream.next())
                .await
                .map_err(|_| "The download stalled. Retry to resume it.")?
        {
            let chunk = chunk.map_err(|_| "Model download interrupted. Retry to resume it.")?;
            bytes += chunk.len() as u64;
            if bytes > file.bytes {
                return Err("Model exceeded the expected download size.".into());
            }
            hash.update(&chunk);
            out.write_all(&chunk)
                .await
                .map_err(|_| "Couldn't save the downloaded model.")?;
            total += chunk.len() as u64;
            let _ = app.emit(
                "download-progress",
                serde_json::json!({"percent":(total as f64/model.bytes as f64*100.0).min(100.0)}),
            );
        }
        out.sync_all()
            .await
            .map_err(|_| "Couldn't finish writing the model.")?;
        if bytes != file.bytes || hex::encode(hash.finalize()) != file.sha256.to_lowercase() {
            let _ = std::fs::remove_file(path);
            return Err("The model couldn't be verified. Retry to download a clean copy.".into());
        }
    }
    std::fs::rename(staging, target).map_err(|_| "Couldn't finish installing the model.")?;
    Ok(())
}

type RequestGuard = Option<(std::sync::Arc<crate::Core>, u64)>;
pub async fn translate(app: &tauri::AppHandle, s: &Settings, text: &str) -> Result<String, String> {
    translate_guarded(app, s, text, None).await
}
pub async fn translate_auto(
    app: &tauri::AppHandle,
    s: &Settings,
    text: &str,
    core: std::sync::Arc<crate::Core>,
    generation: u64,
) -> Result<String, String> {
    translate_guarded(app, s, text, Some((core, generation))).await
}
async fn translate_guarded(
    app: &tauri::AppHandle,
    s: &Settings,
    text: &str,
    guard: RequestGuard,
) -> Result<String, String> {
    if !s.consent {
        return Err("Enable translation after reviewing the privacy notice.".into());
    }
    let path = available_model(app)?;
    run(app, &path, s, text, guard).await
}
fn available_model(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    #[cfg(debug_assertions)]
    if std::env::args().any(|a| a == "--qa-offline") {
        if let Ok(path) = std::env::var("CLARO_QA_MODEL") {
            return Ok(path.into());
        }
    }
    if !licensed() {
        return Err("Activate your offline license in Settings first.".into());
    }
    let model = license::release()
        .model
        .ok_or("The offline model isn't configured in this development build.")?;
    let path = model_dir(app).join(model.id);
    if !path.join("model.bin").exists() {
        return Err("Download your offline model in Settings first.".into());
    }
    Ok(path)
}
static POLICY_EPOCH: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn configure(app: tauri::AppHandle, s: Settings) {
    let epoch = POLICY_EPOCH.fetch_add(1, std::sync::atomic::Ordering::SeqCst) + 1;
    let keep = s.mode == "offline" && s.keep_model_warm;
    KEEP_WARM.store(keep, std::sync::atomic::Ordering::SeqCst);
    tauri::async_runtime::spawn(async move {
        if POLICY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) != epoch {
            return;
        }
        if s.mode != "offline" {
            let mut slot = WORKER.lock().await;
            if POLICY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch {
                slot.take();
            }
            return;
        }
        if keep {
            if let Ok(path) = available_model(&app) {
                let mut slot = WORKER.lock().await;
                if slot.is_none() {
                    if let Ok(worker) = start_worker(&app, &path).await {
                        if POLICY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch {
                            *slot = Some(worker);
                        }
                    }
                }
            } else {
                let mut slot = WORKER.lock().await;
                if POLICY_EPOCH.load(std::sync::atomic::Ordering::SeqCst) == epoch {
                    slot.take();
                }
            }
        } else {
            schedule_idle_unload();
        }
    });
}
fn should_unload(keep: bool, idle: std::time::Duration) -> bool {
    !keep && idle >= std::time::Duration::from_secs(60)
}
fn schedule_idle_unload() {
    if KEEP_WARM.load(std::sync::atomic::Ordering::SeqCst) {
        return;
    }
    tauri::async_runtime::spawn(async {
        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        let mut slot = WORKER.lock().await;
        if slot
            .as_ref()
            .map(|w| {
                should_unload(
                    KEEP_WARM.load(std::sync::atomic::Ordering::SeqCst),
                    w.last_used.elapsed(),
                )
            })
            .unwrap_or(false)
        {
            slot.take();
        }
    });
}

fn language_code(name: &str) -> Option<&'static str> {
    Some(match name {
        "Spanish" => "es",
        "English" => "en",
        "French" => "fr",
        "German" => "de",
        "Italian" => "it",
        "Portuguese" => "pt",
        "Dutch" => "nl",
        "Polish" => "pl",
        "Ukrainian" => "uk",
        "Russian" => "ru",
        "Arabic" => "ar",
        "Hindi" => "hi",
        "Chinese" => "zh",
        "Japanese" => "ja",
        "Korean" => "ko",
        "Turkish" => "tr",
        "Swedish" => "sv",
        "Danish" => "da",
        "Norwegian" => "no",
        "Finnish" => "fi",
        "Czech" => "cs",
        "Greek" => "el",
        "Romanian" => "ro",
        "Hungarian" => "hu",
        "Indonesian" => "id",
        "Vietnamese" => "vi",
        "Thai" => "th",
        "Hebrew" => "he",
        _ => return None,
    })
}
fn detected_code(text: &str) -> Result<&'static str, String> {
    let info = whatlang::detect(text)
        .ok_or("Couldn't detect this language. Choose a source language in Settings.")?;
    if info.confidence() < 0.6 {
        return Err("This selection is ambiguous. Choose a source language in Settings.".into());
    }
    let code = match info.lang().code() {
        "eng" => "en",
        "spa" => "es",
        "fra" => "fr",
        "deu" => "de",
        "ita" => "it",
        "por" => "pt",
        "nld" => "nl",
        "pol" => "pl",
        "ukr" => "uk",
        "rus" => "ru",
        "ara" => "ar",
        "hin" => "hi",
        "cmn" => "zh",
        "jpn" => "ja",
        "kor" => "ko",
        "tur" => "tr",
        "swe" => "sv",
        "dan" => "da",
        "nob" => "no",
        "fin" => "fi",
        "ces" => "cs",
        "ell" => "el",
        "ron" => "ro",
        "hun" => "hu",
        "ind" => "id",
        "vie" => "vi",
        "tha" => "th",
        "heb" => "he",
        _ => return Err("Choose a supported source language in Settings.".into()),
    };
    Ok(code)
}
#[cfg(offline_engine)]
fn engine(app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    let root = app
        .path()
        .app_cache_dir()
        .map_err(|_| "Couldn't locate engine storage.")?
        .join("offline-engine");
    std::fs::create_dir_all(&root).map_err(|_| "Couldn't prepare the offline engine.")?;
    let runner = root.join(if cfg!(windows) {
        "claro-offline.exe"
    } else {
        "claro-offline"
    });
    let library = root.join(if cfg!(windows) {
        "ctranslate2.dll"
    } else {
        "libctranslate2.dylib"
    });
    for (path, bytes) in [
        (
            &runner,
            include_bytes!(env!("CLARO_OFFLINE_RUNNER")).as_slice(),
        ),
        (
            &library,
            include_bytes!(env!("CLARO_OFFLINE_LIBRARY")).as_slice(),
        ),
    ] {
        if std::fs::read(path)
            .map(|existing| existing != bytes)
            .unwrap_or(true)
        {
            let temp = path.with_extension("partial");
            std::fs::write(&temp, bytes).map_err(|_| "Couldn't install the offline engine.")?;
            std::fs::rename(temp, path)
                .map_err(|_| "Couldn't finish installing the offline engine.")?;
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&runner, std::fs::Permissions::from_mode(0o700))
            .map_err(|_| "Couldn't make the offline engine executable.")?;
    }
    Ok(runner)
}
#[cfg(not(offline_engine))]
fn engine(_app: &tauri::AppHandle) -> Result<std::path::PathBuf, String> {
    Err("The native offline engine is not bundled in this development build.".into())
}
struct Worker {
    _child: tokio::process::Child,
    input: tokio::process::ChildStdin,
    output: tokio::io::BufReader<tokio::process::ChildStdout>,
    model: std::path::PathBuf,
    last_used: std::time::Instant,
    request: u64,
}
static KEEP_WARM: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(true);
static WORKER: tokio::sync::Mutex<Option<Worker>> = tokio::sync::Mutex::const_new(None);
fn current(guard: &RequestGuard) -> bool {
    guard
        .as_ref()
        .map(|(core, id)| core.tracker.lock().unwrap().is_current(*id))
        .unwrap_or(true)
}
async fn start_worker(app: &tauri::AppHandle, path: &std::path::Path) -> Result<Worker, String> {
    use tokio::io::AsyncBufReadExt;
    let runner = engine(app)?;
    let mut command = tokio::process::Command::new(runner);
    command
        .arg("--server")
        .arg(path)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    #[cfg(windows)]
    command.creation_flags(0x08000000);
    let mut child = command
        .spawn()
        .map_err(|_| "Couldn't start the offline engine.")?;
    let input = child.stdin.take().unwrap();
    let mut output = tokio::io::BufReader::new(child.stdout.take().unwrap());
    let mut ready = String::new();
    tokio::time::timeout(
        std::time::Duration::from_secs(45),
        output.read_line(&mut ready),
    )
    .await
    .map_err(|_| "The offline model took too long to load.")?
    .map_err(|_| "Couldn't load the offline model.")?;
    let ready: serde_json::Value =
        serde_json::from_str(&ready).map_err(|_| "The offline engine couldn't load this model.")?;
    if ready["ready"] != true {
        return Err("The offline engine couldn't load this model.".into());
    }
    Ok(Worker {
        _child: child,
        input,
        output,
        model: path.into(),
        last_used: std::time::Instant::now(),
        request: 0,
    })
}
async fn run(
    app: &tauri::AppHandle,
    path: &std::path::Path,
    s: &Settings,
    text: &str,
    guard: RequestGuard,
) -> Result<String, String> {
    use tokio::io::AsyncBufReadExt;
    if text.trim().is_empty() || text.len() > 12000 {
        return Err("Select between 1 and 12,000 bytes of text.".into());
    }
    let target = language_code(&s.target).ok_or("Unsupported target language.")?;
    let source = if s.source_language == "Auto" {
        detected_code(text)?
    } else {
        language_code(&s.source_language).ok_or("Unsupported source language.")?
    };
    if source == target {
        return Ok(text.into());
    }
    if let Some((core, _)) = &guard {
        crate::qa(app, core, "offline_queue_entered");
    }
    let mut slot = WORKER.lock().await;
    // Older requests queued behind inference are discarded before doing more work.
    if !current(&guard) {
        return Err("Selection changed.".into());
    }
    if slot
        .as_ref()
        .map(|w| w.model.as_path() != path)
        .unwrap_or(false)
    {
        slot.take();
    }
    if slot.is_none() {
        *slot = Some(start_worker(app, path).await?);
    }
    if !current(&guard) {
        return Err("Selection changed.".into());
    }
    if let Some((core, _)) = &guard {
        crate::qa(app, core, "offline_inference_started");
    }
    let worker = slot.as_mut().unwrap();
    worker.request += 1;
    let id = worker.request;
    let request =
        serde_json::json!({"id":id,"text":text,"target":target,"source":source}).to_string() + "\n";
    let operation = async {
        worker
            .input
            .write_all(request.as_bytes())
            .await
            .map_err(|_| "The offline engine stopped unexpectedly.")?;
        worker
            .input
            .flush()
            .await
            .map_err(|_| "The offline engine stopped unexpectedly.")?;
        let mut line = String::new();
        let read = worker
            .output
            .read_line(&mut line)
            .await
            .map_err(|_| "The offline engine stopped unexpectedly.")?;
        if read == 0 {
            return Err("The offline engine stopped unexpectedly.");
        }
        let response: serde_json::Value = serde_json::from_str(&line)
            .map_err(|_| "The offline engine returned an invalid response.")?;
        if response["id"] != id {
            return Err("The offline engine returned an out-of-order response.");
        }
        Ok(response)
    };
    let response = match tokio::time::timeout(std::time::Duration::from_secs(45), operation).await {
        Ok(Ok(value)) => value,
        Ok(Err(error)) => {
            slot.take();
            return Err(error.into());
        }
        Err(_) => {
            slot.take();
            return Err("Offline translation took too long. Try a shorter selection.".into());
        }
    };
    if current(&guard) {
        if let Some((core, _)) = &guard {
            crate::qa(app, core, "offline_inference_completed");
        }
    }
    // Always drain the matching reply, even if this selection was superseded.
    slot.as_mut().unwrap().last_used = std::time::Instant::now();
    drop(slot);
    schedule_idle_unload();
    if !current(&guard) {
        return Err("Selection changed.".into());
    }
    match response["code"].as_u64() {
        Some(0) => response["translation"]
            .as_str()
            .map(String::from)
            .ok_or("The offline engine returned no translation.".into()),
        Some(7) => {
            Err("This paragraph is too long for the offline model. Try a shorter passage.".into())
        }
        _ => Err("The offline engine couldn't translate this selection.".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn package_paths_and_sizes_are_bounded() {
        let file = license::ModelFile {
            name: "model.bin".into(),
            url: "https://example.invalid/model".into(),
            sha256: "a".repeat(64),
            bytes: 3,
        };
        let mut model = license::Model {
            id: "m2m100-int8-v1".into(),
            files: vec![file],
            bytes: 3,
        };
        assert!(validate_manifest(&model).is_ok());
        model.id = "../escape".into();
        assert!(validate_manifest(&model).is_err());
        model.id = "okay".into();
        model.files[0].name = "../outside".into();
        assert!(validate_manifest(&model).is_err());
        model.files[0].name = "model.bin".into();
        model.bytes = 4;
        assert!(validate_manifest(&model).is_err());
    }
    #[test]
    fn language_detection_has_manual_escape_hatch() {
        assert_eq!(language_code("Spanish"), Some("es"));
        assert_eq!(
            detected_code("This is a complete English sentence that should provide enough context for the automatic language detector to identify it reliably.").unwrap(),
            "en"
        );
        assert_eq!(
            detected_code("Me gustaría saber dónde está la estación de tren porque tengo que viajar esta mañana y no conozco esta ciudad.").unwrap(),
            "es"
        );
        assert!(detected_code("x").is_err());
    }
}

#[cfg(test)]
mod policy_tests {
    use super::*;
    #[test]
    fn warm_is_default_and_can_be_disabled() {
        let settings = Settings::default();
        assert!(settings.keep_model_warm);
        let mut json = serde_json::to_value(&settings).unwrap();
        json.as_object_mut().unwrap().remove("keepModelWarm");
        assert!(
            serde_json::from_value::<Settings>(json.clone())
                .unwrap()
                .keep_model_warm
        );
        json["keepModelWarm"] = serde_json::json!(false);
        assert!(
            !serde_json::from_value::<Settings>(json)
                .unwrap()
                .keep_model_warm
        );
    }
    #[test]
    fn warm_model_is_not_evicted_and_cold_mode_has_a_real_idle_bound() {
        use std::time::Duration;
        assert!(!should_unload(true, Duration::from_secs(3600)));
        assert!(!should_unload(false, Duration::from_secs(59)));
        assert!(should_unload(false, Duration::from_secs(60)));
    }
}
