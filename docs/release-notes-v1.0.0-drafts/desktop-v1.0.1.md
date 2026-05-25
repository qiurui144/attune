# Attune Desktop v1.0.1 (TBD, ~2026-05-27–31)

> Desktop installer patch release alongside server/CLI v1.0.1.
> No breaking changes. All v1.0.0 desktop users are encouraged to upgrade.

---

## New in Desktop

- **In-app auto-updater** — Desktop builds now check for updates on startup and surface a native OS dialog when a new release is available. Users can install with one click without visiting the download page. Powered by Tauri's updater protocol; update signature verification enforced (Ed25519). See `docs/tauri-updater-deploy.md` for self-hosted update server setup.
- **WinGet support** — `attune` package available via `winget install attune`. Manifest auto-submitted to [winget-pkgs](https://github.com/microsoft/winget-pkgs) on each GA tag.
- **APT / RPM repository** — Linux users can now add the attune package repository for automatic upgrades via `apt upgrade` or `dnf upgrade`.

---

## Bug Fixes (inherited from server v1.0.1)

All bug fixes from the server/CLI v1.0.1 release apply to the embedded server process:

- CLI vault-import false positive (#61)
- OCR gender label / amount thousands-separator (#62)
- parse_llm_terms drift between core and server (#77)
- LLM upstream error status pass-through (429/503/4xx)

---

## Known Issues

- macOS build not yet available; targeting v1.1.
- Auto-updater requires internet access to `releases.attune.ai`; air-gap environments should use the `attune-desktop-installers` OCI image and manual install.

---

## Downloads

*(links populated when tag is pushed)*

| Platform | Artifact |
|----------|----------|
| Windows x86_64 | `attune-desktop-v1.0.1-x86_64-windows.msi` |
| Windows x86_64 (portable) | `attune-desktop-v1.0.1-x86_64-windows-setup.exe` (NSIS) |
| Linux x86_64 | `attune-desktop-v1.0.1-x86_64-linux.deb` |
| Linux x86_64 (portable) | `attune-desktop-v1.0.1-x86_64-linux.AppImage` |
| Linux aarch64 | `attune-desktop-v1.0.1-aarch64-linux.deb` |
| Air-gap bundle | `ghcr.io/qiurui144/attune-desktop-installers:1.0.1` |
