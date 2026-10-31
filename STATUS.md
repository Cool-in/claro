# Claro current status

Updated 2026-10-31. Private source, version 0.1.0, macOS arm64 development machine, stable Xcode 27. No public release or paid product has been created.

## Implemented and checked

- TypeScript production bundle builds successfully. npm dependency audit reports zero vulnerabilities.
- 13 serial Rust tests pass: provider URL/security validation, direct HTTP request/response contract, provider errors and truncation, privacy/size gate, Polar public API schema/header/product identity, model manifest paths/sizes, language detection/manual override and selection generation/dismiss/clear guards.
- A macOS debug `.app` bundle was built and launched. Native helper process launch was observed. macOS GUI screenshot and live interaction checks are blocked by the locked desktop; none is claimed passed.
- Native CPU offline engine really translates with the official converted M2M100 int8 model. A persistent worker preserves request IDs and recovers after an oversized paragraph error. Cold sample 1.394 s; warm samples 0.253 s and 0.269 s; next request after rejection 0.245 s. Warm worker RSS 655 MiB. These are synthetic fixtures, excluding the 400 ms selection debounce.
- Runtime tokenizer adds M2M100 source and target language special tokens explicitly. Paragraph boundaries and same-language input are preserved. Oversized paragraphs fail without returning partial text.
- Current model, native worker and reusable build caches live under `/Users/colin/Library/Developer/BuildCaches/Claro`. No model weights are committed.

## Gates still open

- Optimized release idle-process measurement and final bundle verification are in progress.
- Live macOS selection capture, permission flow, focus retention, appearance, tray controls and launch-at-login need an unlocked desktop.
- Windows native compilation is prepared as a private manually dispatched GitHub Actions job; execution and runtime acceptance are pending.
- Real cloud-provider request needs the user's API key. Equivalent provider HTTP integration tests passed.
- Real Polar purchase/license flow and model download need the actual product/benefit/checkout identifiers and approved immutable model URLs. They are explicitly disabled while missing.
- macOS distribution signing/notarization and Windows installer signing are separate release gates.
