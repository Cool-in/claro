# Changelog

## Unreleased

- Skips selections with nothing to translate (numbers, symbols, a lone link or email address) and longer passages already in the target language, so they never cost a provider request.
- Works with models that reject `max_tokens` or a custom `temperature`: a 400 is retried once in the newer request shape and remembered for that provider and model.
- Provider errors now include the provider's own short message, and a missing model or URL gets a clear hint.
- Remembered translations appear at once, with no debounce or loading state.
- Re-opens the provider connection when you start interacting after an idle spell.
- The selection reader restarts itself if it stops, and reports clearly if it keeps failing.
- The tray menu says Pause Claro or Resume Claro to match the current state.
- Manual translation gains a Copy button and Cmd or Ctrl + Enter.
- The popup no longer asks the native side to resize on every streamed token.

## 0.2.0

Faster API mode, more control, and fixes found in an audit.

### Faster
- Translations stream into the popup word by word instead of arriving all at once. Providers that do not stream still work.
- The connection to your provider is opened ahead of time, so the first selection skips the handshake. Nothing is sent but an empty request.
- The last 48 translations are remembered in memory only, so selecting the same text again is instant. Nothing is written to disk and it is cleared when Claro quits.
- Your API key and license are read from the system credential store once and then held in memory, instead of on every translation and every Settings refresh.

### New
- Tone setting for API mode: Neutral, Formal or Casual.
- Quick setup menu in Your provider for OpenAI, Groq, OpenRouter and Ollama. Model names are suggestions you can edit.
- Settings follows the system light or dark appearance.

### Fixed
- The source language setting is now sent to your provider in API mode. It was ignored before.
- The 12,000 limit counts characters, not bytes. Chinese, Japanese and Korean selections of a few thousand characters were rejected.
- A 400 response from Polar no longer removes a saved license. Only 403 and 404 do.
- Small grey text is darker and larger for readability.

### Pricing
- The offline model price shown in the app now comes from `priceLabel` in `release.json` and reads €10. Add a EUR price to the Polar product before shipping so checkout matches.
