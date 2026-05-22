# Attune Desktop v1.0.0 — Native Desktop App (2026-05-25 GA)

> Tauri-powered desktop app for Attune — full 8-tab Web UI, system tray, auto-updater, bundled server.
> Pairs with server/CLI `v1.0.0`. Same vault, same encryption, native install experience.

---

## Highlights

- **5 installer formats across 2 platforms** — Windows (NSIS exe + MSI) and Linux (deb + RPM + AppImage)
- **Tauri auto-updater** — NSIS (Windows) and AppImage (Linux) bundles are signed; desktop notifies and applies updates automatically
- **Bundled attune-server** — no separate install needed; server starts on port 18900 alongside the desktop app
- **8-tab UI** — Chat / Knowledge / Timeline / Files / Office / Skills / Marketplace / Settings
- **First-launch Wizard** — 5-step guided setup: password → data directory → LLM → hardware → ingest sources

---

## Downloads

| Platform | Installer | Size (approx.) | Notes |
|----------|-----------|----------------|-------|
| **Windows x86_64** | `Attune_1.0.0_x64-setup.exe` | ~150 MB | NSIS; supports auto-updater (`*.sig` included) |
| **Windows x86_64** | `Attune_1.0.0_x64.msi` | ~150 MB | MSI; enterprise deployment via GPO; no auto-updater |
| **Linux x86_64** | `attune_1.0.0_amd64.deb` | ~120 MB | Ubuntu 20.04+ / Debian 11+; systemd service auto-configured |
| **Linux x86_64** | `attune_1.0.0_x86_64.rpm` | ~120 MB | Fedora / RHEL / openSUSE |
| **Linux x86_64** | `Attune_1.0.0_amd64.AppImage` | ~130 MB | Portable; supports auto-updater (`*.sig` included) |

> **macOS**: not in v1.0.0 desktop artifacts. macOS Apple Silicon and Intel users can run the server tarball (`attune-v1.0.0-macos-aarch64.tar.gz`) from the server release, and access the Web UI at `http://127.0.0.1:18900`. Native macOS `.dmg` is planned for a future release.
>
> **Linux ARM64 (aarch64)**: desktop installer not included. Server + CLI tarball `attune-v1.0.0-linux-aarch64.tar.gz` available in the server release (covers K3 appliance, Raspberry Pi, ARM NAS).

SHA256 checksums: see `checksums.txt` attached to this release. Verify before installing:

```bash
# Linux
sha256sum -c checksums.txt

# Windows (PowerShell)
Get-FileHash Attune_1.0.0_x64-setup.exe -Algorithm SHA256
```

---

## What's New (desktop-v0.7.0 → desktop-v1.0.0)

### New in Desktop

- **OfficeView tab** — OCR image panel (drag & drop or file picker) + ASR meeting transcription panel with real-time WebSocket progress bar; supports 5 OCR scene profiles and 3 card subtypes
- **Plugin Marketplace tab** — browse, install, and manage attune-pro plugin packs; shows entitlement status after cloud login
- **Cloud Member panel** (Settings → Member) — login / logout / license status / sync-plugins button
- **LLM gateway auto-configure** — after `attune login`, the running server hot-reloads the cloud gateway URL + token; no manual restart needed
- **Tauri auto-updater infra** — update channel configured; in-app notification when a new version is available
- **OCI image for installers** — `ghcr.io/qiurui144/attune-desktop-installers:latest` published for automated downstream consumption

### Carried from v0.7.1 Office Helper

- `attune ocr` / `attune transcribe` CLI commands (run from terminal alongside the desktop app)
- Structured OCR: receipt / table / card / id_card / document scenes
- Async ASR: whisper.cpp job queue + WebSocket progress

### Under the Hood

- Tauri `tauri.conf.json` version bumped to `1.0.0` for GA
- Rust builder image bumped 1.88 → 1.91; runtime base switched to `ubuntu:24.04` (glibc 2.39) for broader compatibility
- `FORCE_JAVASCRIPT_ACTIONS_TO_NODE24` set in CI to match updated GitHub Actions runner

---

## Installation

### Windows

1. Download `Attune_1.0.0_x64-setup.exe`
2. Run the installer (UAC prompt is normal — installs to `%LOCALAPPDATA%\Programs\Attune`)
3. Attune launches automatically; system tray icon appears in the taskbar notification area
4. First-launch wizard guides you through vault creation (5 steps, ~2 minutes)

> **Enterprise / IT admins**: use `Attune_1.0.0_x64.msi` for GPO deployment. Silent install: `msiexec /i Attune_1.0.0_x64.msi /quiet`

### Linux (deb)

```bash
sudo dpkg -i attune_1.0.0_amd64.deb
# OR
sudo apt install ./attune_1.0.0_amd64.deb  # resolves dependencies automatically

attune-desktop   # launch GUI
# or: find Attune in your application menu
```

### Linux (AppImage)

```bash
chmod +x Attune_1.0.0_amd64.AppImage
./Attune_1.0.0_amd64.AppImage
```

---

## Upgrade from desktop-v0.7.x

The installer overwrites the previous version. Your vault is stored separately at:

- **Windows**: `%APPDATA%\attune\vault\`
- **Linux**: `~/.local/share/attune/vault/`

Vault is **not touched** by the installer/uninstaller. Upgrade steps:

1. (Recommended) Backup vault: `attune export --output ~/attune-backup.zip`
2. Run the new installer — it will overwrite the previous desktop app
3. Launch and unlock with your existing password — vault schema auto-migrates (< 2 seconds)

---

## Known Limitations

- **macOS** not in this release (see Downloads section above)
- **Linux ARM64 desktop** not in this release (server/CLI tarball available)
- MSI bundle does not include auto-updater signature — use the NSIS exe for auto-update support
- First-launch wizard Step 4 (hardware detection) may show "Unknown" on non-standard GPU configurations; Ollama will still work with CPU fallback

---

## Documentation

- Install guide: https://wiki.attune.ai/attune/install
- Desktop app docs: https://wiki.attune.ai/attune/desktop
- Source (Tauri config): `rust/crates/attune-desktop/` in https://github.com/qiurui144/attune
- Issues: https://github.com/qiurui144/attune/issues
