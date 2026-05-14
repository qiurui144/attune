# Bundled Binaries

This directory contains third-party binaries that get shipped inside the .deb / NSIS installer
as Tauri bundle resources. They land at `/usr/lib/Attune/bin/` after install and are symlinked
to `/usr/local/bin/` by the postinst hook.

## whisper-cli

| | |
|---|---|
| **Project** | [ggml-org/whisper.cpp](https://github.com/ggml-org/whisper.cpp) |
| **License** | MIT |
| **Build target** | x86_64-linux, statically linked (libwhisper, libggml) |
| **Dynamic deps** | only system libc / libstdc++ / libgomp |
| **Size** | ~2.6 MB |

### Rebuild

When upstream releases a new version, rebuild via `scripts/build-whisper-cli.sh`:

```bash
bash apps/attune-desktop/resources/bin/build-whisper-cli.sh
```

The script:
1. Clones whisper.cpp at HEAD (or pinned tag)
2. Compiles with `BUILD_SHARED_LIBS=OFF GGML_NATIVE=OFF GGML_AVX2=OFF` for broad CPU compat
3. Copies the resulting `whisper-cli` binary into this directory

CI (`.github/workflows/desktop-release.yml`) should call this script before `cargo tauri build`
to ensure each release ships a fresh binary.

## Why bundle instead of apt-install?

Ubuntu/Debian don't have whisper.cpp in their official repos as of 2026. Available options:
- **Bundle (current choice)**: ship a known-good static binary, ~2.6 MB
- **PPA**: third-party trust + dependency hell
- **Compile in postinst**: requires `build-essential` + 5 min on user machine, fragile
- **Use snap/flatpak**: another runtime — defeats "double-click and ready"

Bundling is the smallest user-visible cost.
