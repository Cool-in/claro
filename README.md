# Claro

A quiet desktop translator for macOS and Windows. Highlight text in another app and a small, nonactivating panel appears beside the selection. Spanish is the default target language. Source detection and a manual source override are available.

Free download with your own OpenAI or OpenAI-compatible API key. A planned $10 lifetime Polar purchase unlocks an optional local translation model on both platforms. This repository is private. No workflow publishes source or creates a public release.

## What is implemented

- A compact content-sized popup with one header, bundled target-language flags, copy/dismiss and immediate retranslation when its target changes. The target becomes the saved default. Drag the blank header/Claro mark to retain a position; the menu’s Follow selection option resets it. Only coordinates are saved.
- Rust/Tauri 2 background core, tray menu and shared TypeScript UI. Settings and popup webviews are created when needed. Closing settings destroys its webview. A dismissed popup is destroyed after 10 seconds.
- Native macOS Accessibility notifications, mouse-up wake and bounded 600 ms fallback checks. Native Windows UI Automation TextPattern, selection/focus events and bounded fallback checks. No clipboard scraping or shell-based text capture.
- A 60 ms mouse-up settle plus 30 ms dispatch delay, 150 ms keyboard debounce, current-selection deduplication, stale-result guards, cancellation of obsolete automatic cloud requests, pause, app exclusions, secure-field exclusion and lock/session suspension. Unsupported apps have a manual paste path.
- Direct provider HTTPS calls, with loopback HTTP allowed for a local provider. API keys are stored per provider URL in macOS Keychain or Windows Credential Manager. Provider response bodies and selected text are never logged or persisted as history.
- A compiled, CPU-only CTranslate2/SentencePiece offline worker. It prewarms and keeps the unlocked/downloaded model ready by default in offline mode, serializes requests and matches response IDs. A memory-saving preference unloads it after 60 idle seconds; switching to API mode releases it. End users do not need Python.
- Explicit resumable model download, pinned SHA-256 and byte-size checks, disk-space checks and atomic installation. No automatic cloud fallback.
- Polar's public customer license validation API, expected organization/benefit checks and a secure cached entitlement. Network failure preserves a previously verified lifetime license; a definitive invalid/revoked response removes it. No organization token is bundled.
- Launch at login is optional and off by default.

## Local development

Requires Node 22+, stable Rust, and the native platform toolchain: stable Xcode on Mac; Visual Studio C++ build tools on Windows. The current macOS bundle targets macOS 26+. Windows targets x64. GUI acceptance still needs an unlocked Mac and an actual Windows machine.

```sh
npm ci
npm run build
npm test
npm run tauri -- dev
```

On this Mac, reuse `/Users/colin/Library/Developer/BuildCaches/Claro/macos/debug/Claro` as `CARGO_TARGET_DIR`. Check disk headroom before builds. `npm test` uses that stable location automatically on macOS unless the environment overrides it. Keep serial Rust tests.

Build a bundle with the offline worker embedded:

```sh
# cmake and git must be available; on Windows use an MSVC developer shell.
python3 scripts/build-offline.py --cache /absolute/stable/Claro/macos --jobs 4
export CLARO_OFFLINE_DIR=/absolute/stable/Claro/macos/offline-native/runner
export CARGO_TARGET_DIR=/absolute/stable/Claro/macos/release/Claro
npm run tauri -- build --bundles app
# Windows uses: npm run tauri -- build --bundles nsis
```

The native build script pins CTranslate2 4.8.4 and SentencePiece 0.2.1 to exact commits. On Windows it produces `claro-offline.exe` plus `ctranslate2.dll`; on macOS it produces `claro-offline` plus `libctranslate2.dylib`. Both are embedded into the desktop binary and installed into its cache when offline inference is first used. Platform system libraries are the only other runtime dependencies.

Use `CLARO_SIGN_IDENTITY` while building native binaries and the desktop app to sign embedded macOS executables before embedding. The outer app still needs its signing identity configured for distribution. A local build is not evidence of notarization or Windows installer signing.

`.github/workflows/build.yml` is a manually dispatched Windows build with CPU-only native dependencies, Rust tests and an NSIS installer artifact. It does not publish a release. macOS is built locally to reuse the user's stable Xcode and build cache.

## Model provenance and release configuration

`offline/model.recipe.json` records the official Meta model revision and conversion tool versions. The initial model is M2M100 418M, quantized to int8. The current converted runtime files total approximately 473 MiB. They are not checked into Git.

For a new conversion, use build-time Python tooling, then ship only the native runtime/model files:

```sh
ct2-transformers-converter --model facebook/m2m100_418M \
  --revision 55c2e61bbf05dfb8d7abccdc3fae6fc8512fd636 \
  --output_dir /stable/Claro/offline-model/m2m100-int8 \
  --quantization int8 \
  --copy_files sentencepiece.bpe.model tokenizer_config.json vocab.json special_tokens_map.json
python3 scripts/model-manifest.py /stable/Claro/offline-model/m2m100-int8 \
  --base-url https://YOUR-APPROVED-MODEL-DISTRIBUTION-URL
```

Check the converter's current `--help` before use; the exact pinned source revision must be selected. `offline/model.manifest.reference.json` contains the verified local file checksums and explicitly nonfunctional placeholder URLs. It is a reference, not an enabled download manifest.

Before enabling purchase/download in a release, populate `src-tauri/release.json` with the real Polar organization ID, license-key benefit ID, public checkout URL and model download manifest. Configure a $10 lifetime product without expiry or usage quotas. A single license must work on Mac and Windows; device activations are not required by this client. Keep all organization tokens and signing secrets outside the repository.

The Polar product does not exist yet, so checkout, real entitlement activation and public model download remain deliberately unavailable. No fake purchase or license is created. Automatic cloud translation also requires the user's own provider key.

## Performance and verification

See `STATUS.md` for current build identity, measurements and passed checks; `ISSUES.md` lists acceptance gates. Run `scripts/benchmark-offline.py RUNNER MODEL_DIRECTORY` for a synthetic cold/warm IPC benchmark. On this Apple M1 Max with 32 GiB RAM, earlier short-sentence samples were about 1.4 seconds cold and 0.25 seconds warm. A later run during heavy host load (load average about 23) measured 1.1–2.35 seconds warm. Worker RSS varied from about 630 to 785 MiB across runs. These are engine-only synthetic samples, excluding native selection, popup creation and any provider/network latency. No comparative evidence supports a “fastest translator” claim. Offline mode is an optional larger download and active-memory cost. The default keeps the model warm; turn off “Keep the model ready” to free it after one idle minute.

Ambiguous short offline inputs request a source-language override. Paragraphs over 510 source tokens and capped responses are rejected with a clear shorter-passage message rather than silently truncated. This initial model can lose nuance; translation correctness is not guaranteed by a successful runtime call.

Debug-only flags `--preview-popup`, `--qa-diagnostics` and `--qa-offline` exist for visual/runtime QA. Preview text is a synthetic fixture. `--qa-offline` additionally requires `CLARO_QA_MODEL` pointing to an already converted local model and bypasses entitlement only in debug builds. `--qa-diagnostics` writes only current counters/timestamps and state flags to the app cache, with no text, translation, key or app identifiers. Release builds contain no entitlement bypass or diagnostic-file writes. `--background` starts only the tray and selection reader, useful for login launch and idle measurements.

Sources: [Tauri](https://v2.tauri.app/), [M2M100](https://huggingface.co/facebook/m2m100_418M), [CTranslate2](https://opennmt.net/CTranslate2/guides/transformers.html), [Polar license validation](https://polar.sh/docs/api-reference/2026-10/customer_portal/validate-license-key). Third-party runtime licenses are in `offline/licenses`.
