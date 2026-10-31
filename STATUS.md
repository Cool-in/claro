# Claro current status

Updated 2026-10-31. Version 0.1.0, private repository https://github.com/Cool-in/claro. Apple M1 Max, 32 GiB RAM, macOS 26.6.2 arm64, stable Xcode 27. No public release has been created. The private/unlisted $10 lifetime Polar product and license benefit are configured; checkout is still in test mode with payments unavailable.

## Passed

- TypeScript production bundle builds. npm audit reports zero vulnerabilities.
- 20 serial Rust tests pass: provider URL/security and real localhost HTTP request/response contracts, provider errors/truncation/privacy/size gates, Polar public API schema/header/product identity, manifest paths/sizes, source detection/manual override, selection generation/dismiss/clear/mouse-settle guards persisted warm-model default/opt-out/idle policy renderer-ready generation gating current-selection retarget/clear behavior and backward-compatible persisted popup themes.
- Live macOS UI review passed at fixed 650 × 600: Translation, Provider, Offline and Manual pages have all controls visible without a page scrollbar. Offline warm checkbox is checked by default. Notices have a reserved bottom area. The licensed model/download card is shorter than the observed unlicensed purchase/license block; its real purchase state is unavailable until Polar is configured.
- Real manual offline translation through the desktop UI passed: a synthetic English request for the nearest train station produced Spanish output using the compiled local worker. This proves the runtime path, not editorial translation quality.
- Real automatic capture/translation passed by user confirmation: selected English text produced the Spanish popup.
- Compact popup QA passed: short “Buen día.” card measured 188 × approximately 85 pixels, one header and no footer. All target languages were accessible through the menu. Choosing French ran the actual offline worker (“Bonne matinée !”); returning to Spanish also translated successfully, and the persisted default read back as Spanish. The Follow selection reset and dismiss controls passed AX interaction.
- After the main-thread fix, the app/helper/worker survived closing Settings. macOS Reopen restores Settings when no window is visible.
- Windows CI 41204352706 passed for commit `3663e9c` and produced an NSIS installer for the compact/retarget/movable checkpoint. The latest mouse/theme changes are newer than that checkpoint; Windows runtime acceptance is still unverified.
- Light and Dark popup themes passed visual review; Dark persisted as `popupTheme: dark`. The rounded outer area is transparent and the six-dot grip is visible. System/Light/Dark are selectable from the popup menu.
- Copy passed AX activation followed by actual native TextEdit paste of “Buen día.”; the browser connector’s separate clipboard read was not the OS clipboard. Dismiss passed AX interaction.
- Runtime/model third-party licenses are bundled. The optional current model is 472.9 MiB, pinned by SHA-256; no weights are committed.

## Performance and policy

- Offline mode prewarms a downloaded/unlocked model and keeps it ready by default. “Keep the model ready” can be disabled to unload after one idle minute. API mode releases the local worker.
- Earlier native IPC samples: 1.394 s cold and 0.245–0.269 s warm. Later samples during heavy host load (load average about 23): 1.097–2.345 s warm. Observed worker RSS varied about 630–785 MiB. These measurements exclude capture, popup and cloud/provider latency and do not establish comparative speed.
- Event-driven mouse selection uses 60 ms native settling and 30 ms dispatch delay; keyboard selection debounce is 150 ms. The fallback poll remains bounded at 600 ms. Actual end-to-end automatic latency remains unverified.
- Optimized local .app build passed at 10.38 MiB (10,885,214 bytes), with all three runtime/model notices present. Executable SHA-256: `08566bf8e94d395ca7548c2ac21c3eb96bb9617912880914ed06ac90a9f33764`. This release candidate precedes the latest popup refinements. The latest debug bundle built successfully at about 47.05 MiB.
- Tray-only release measurement (API mode, automatic off, no windows/model): main 72.4 MiB RSS plus reader 32.0 MiB, summed RSS 104.5 MiB. Mean CPU was 0% over five seconds at `ps` timing resolution; no new WebKit process appeared. Summed RSS is not a physical-memory footprint or an active-capture measurement.
- Two superseded task-downloaded original model weight blobs were removed after checking all references, ownership and open files. Verified int8 model, reusable caches and current products were preserved. Actual statvfs free-space increase was 3.574 GiB (76,416,872,448 to 80,254,119,936 bytes).

## Still unverified or externally blocked

- The previous native drag route could return success without moving because asynchronous IPC had no current Cocoa mouse event. It was replaced with pointer capture and native-cursor coordinate updates, preserving non-key display. Inactive-window mouse delivery now explicitly uses `accept_first_mouse(true)`. Physical mouse clicks/dragging remain unverified: CUA coordinate control of the non-key dialog returned “noWindowsAvailable.” AX controls passed; this is not proof of physical pointer behavior.
- Original-editor typing/copy/delete focus, permission revocation, secure fields, per-app exclusions and launch-at-login were not fully replayed. No Accessibility permission was granted by the agent.
- The app is left alive in background offline QA mode for the user: automatic enabled, source English, target Spanish, warm model on and Dark theme persisted. Do not stop it merely to conclude a report.
- Real cloud-provider translation needs the user's API key; local HTTP-equivalent integration passed.
- Polar product `4d211ac8-3549-4f89-8961-4a58368c266d` and benefit `ff9e3867-0943-4e1f-9dcb-059cfb5cee08` are verified as US$10 one-time, never expiring, unlimited activations/usage. Public organization/benefit/checkout configuration is populated. Checkout displays test mode and payments unavailable; no real purchase or license activation has passed. The model manifest remains null, and both UI/backend purchase entry points stay gated until model distribution is configured.
- Windows runtime acceptance and release signing/notarization remain separate gates.
