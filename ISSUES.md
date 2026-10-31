# Claro issue register

| ID | State | Acceptance gate |
| --- | --- | --- |
| QA-MAC | Blocked externally | Desktop is locked. Verify selection in an exposed native text view and browser, denied/revoked Accessibility, secure fields, pause/lock, outside/Escape/app-switch dismissal, no focus steal, popup screenshot and copy. Do not bypass the lock. |
| QA-WIN | Pending | Run the private Windows CI build, then exercise UIA selection and secure fields, mixed-DPI placement, dismissal, permission boundaries and installer on Windows. A compile on Mac is not Windows acceptance. |
| PROVIDER | Blocked externally | Real provider translation requires the user's BYOK secret. The direct HTTP integration path is verified against a local server. |
| POLAR | Blocked externally | User has not created the Polar product. Configure lifetime license benefit and public IDs/checkout, then verify purchase, activation on both OSes, cached offline access and actual revocation. |
| MODEL-DISTRIBUTION | Blocked externally | Publish approved immutable model files with notices and pinned checksums, wire release manifest, then verify interrupted download/resume and atomic install. No public upload is authorized by this source implementation. |
| RELEASE-TRUST | Pending | Sign embedded native executables before embedding, sign/notarize macOS installer, and sign Windows installer. Local development builds are not release acceptance. |
| OFFLINE-LIMITS | Known behavior | Optional approximately 473 MiB model and approximately 655 MiB active worker RSS; worker unloads after 60 idle seconds. Ambiguous short source text needs an override. Paragraphs above 510 tokens return a shorter-passage error; no silent truncation. |
