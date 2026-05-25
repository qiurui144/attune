use clap::{Parser, Subcommand};
use attune_core::vault::Vault;

#[derive(Parser)]
#[command(name = "attune", version, about = "Attune CLI — Private AI Knowledge Companion")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize vault with a master password
    Setup,
    /// Unlock the vault
    Unlock,
    /// Lock the vault
    Lock,
    /// Show vault status
    Status,
    /// Insert a knowledge item
    Insert {
        #[arg(short, long)]
        title: String,
        #[arg(short, long)]
        content: String,
        #[arg(short, long, default_value = "note")]
        source_type: String,
    },
    /// Get a knowledge item by ID
    Get {
        id: String,
    },
    /// List knowledge items
    List {
        #[arg(short, long, default_value = "20")]
        limit: usize,
    },
    /// Run PP-OCRv5 on an image file (PNG/JPG) and print extracted text.
    /// Useful for testing OCR setup or quick screenshot OCR without going through ingest.
    ///
    /// Exit codes (D5.7):
    ///   0 success | 1 user input (file missing / unknown profile)
    ///   2 reserved (red-line) | 3 engine failure
    Ocr {
        /// Image path (PNG / JPG / etc supported by `image` crate)
        image: std::path::PathBuf,
        /// Office helper scene profile (document/receipt/table/card/id_card).
        /// Default = no structured extraction, plain text output.
        #[arg(long)]
        profile: Option<String>,
        /// For profile=id_card: subtype = id_card_cn | bank_card | business_license
        #[arg(long)]
        id_card_subtype: Option<String>,
        /// Output full JSON envelope (lines + bbox + structured) instead of plain text.
        #[arg(long)]
        json: bool,
        /// Include per-line bbox coordinates in JSON envelope (default: true).
        /// Use `--no-bbox` to strip bbox for compact JSON (lines text-only).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        bbox: bool,
    },
    /// Office helper async ASR transcription. Currently runs synchronously in-process
    /// (no daemon), printing transcript JSON to stdout.
    ///
    /// Exit codes (D5.7):
    ///   0 success | 1 user input (file missing) | 3 engine failure
    Transcribe {
        /// Audio path (mp3/wav/m4a/flac/ogg)
        audio: std::path::PathBuf,
        /// Enable speaker diarization (pyannote / WhisperX subprocess)
        #[arg(long)]
        diarization: bool,
        /// Output as JSON instead of [HH:MM:SS] formatted lines
        #[arg(long)]
        json: bool,
        /// Block until transcription completes (default: true; in-process mode is always sync,
        /// flag reserved for future async/job-id mode where `--no-wait` returns job_id immediately).
        #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
        wait: bool,
    },
    /// R38 (2026-05-01): Export vault data files to a backup directory.
    /// Copies vault.db (encrypted SQLite), tantivy/ (full-text index), vectors.encbin (if present).
    /// Vault MUST be locked first (this command verifies and refuses if unlocked, to avoid WAL corruption).
    /// Note: device.key (master secret) is NOT exported — user must back that up separately.
    VaultExport {
        /// Destination directory (will be created if missing)
        dest: std::path::PathBuf,
    },
    /// R38: Import a previously exported vault into data_dir.
    /// Vault MUST be sealed (no setup yet) OR locked; refuses if any vault.db exists.
    /// Use --force to overwrite (DANGEROUS — will replace current vault).
    VaultImport {
        /// Source directory containing vault.db (and optional tantivy/ + vectors.encbin)
        src: std::path::PathBuf,
        /// Overwrite existing vault data (DANGEROUS)
        #[arg(long)]
        force: bool,
    },
    /// R-deploy (2026-05-01): Linux 一键部署 — 自动安装 Ollama + 硬件自适应 + 拉模型。
    /// 检测 NVIDIA / AMD / CPU；AMD APU 注入 HSA_OVERRIDE_GFX_VERSION；按 RAM 选模型 tier。
    /// 调底层 scripts/deploy-linux.sh（必须与二进制在同一仓库部署）。
    Deploy {
        /// 不拉模型（只装 Ollama runtime + 配 GPU 路径）
        #[arg(long)]
        no_models: bool,
        /// 只打印计划不执行
        #[arg(long)]
        dry_run: bool,
        /// 自定义 deploy script 路径（默认: ./scripts/deploy-linux.sh）
        #[arg(long)]
        script: Option<std::path::PathBuf>,
    },
    /// 加密 plugin.yaml: free → paid 分发前的最后一步.
    /// 输入 plugin dir + key (env ATTUNE_PLUGIN_KEY), 写出 plugin.yaml.enc.
    PluginEncrypt {
        /// 包含 plugin.yaml 的目录
        plugin_dir: std::path::PathBuf,
        /// 加密密钥 (默认从 ATTUNE_PLUGIN_KEY env 读)
        #[arg(long)]
        key: Option<String>,
        /// 加密后是否删除明文 plugin.yaml (默认 false 保留两份方便 diff)
        #[arg(long)]
        delete_plain: bool,
    },
    /// 解密 plugin.yaml.enc → plugin.yaml (用于本地调试 / 验证 key 正确)
    PluginDecrypt {
        plugin_dir: std::path::PathBuf,
        #[arg(long)]
        key: Option<String>,
    },
    /// 验证 plugin 装载链路: 解密 (如有) → schema 解析 → trust↔pricing 联动校验
    PluginVerify {
        plugin_dir: std::path::PathBuf,
        /// paid plugin 的 key (free plugin 不需)
        #[arg(long)]
        key: Option<String>,
        /// 模拟 trust 级别 (默认 Unsigned)
        #[arg(long, default_value = "Unsigned")]
        trust: String,
    },
    /// 生成 Ed25519 keypair (32-byte 私钥 hex + 32-byte 公钥 hex).
    /// 私钥**必须离线安全保管**, 公钥可嵌入 OFFICIAL_PUBLIC_KEYS 或公开发布.
    PluginKeygen {
        /// 私钥写入路径 (默认 stdout)
        #[arg(long)]
        out_priv: Option<std::path::PathBuf>,
    },
    /// 用私钥签名 plugin_dir (写 plugin.sig).
    /// 私钥来源: --priv-key=<hex> 或 env ATTUNE_PLUGIN_SIGN_KEY 或 --priv-file=<path>
    PluginSign {
        plugin_dir: std::path::PathBuf,
        #[arg(long)]
        priv_key: Option<String>,
        #[arg(long)]
        priv_file: Option<std::path::PathBuf>,
    },
    /// 用公钥校验 plugin_dir 的 plugin.sig (与 OFFICIAL_PUBLIC_KEYS 内嵌列表无关)
    PluginVerifySig {
        plugin_dir: std::path::PathBuf,
        /// 公钥 hex (32-byte)
        /// 用法: attune plugin-verify-sig <dir> --pubkey <pubkey>
        #[arg(long)]
        pubkey: String,
    },
    /// 装载 plugin 到 attune 默认 plugins 目录 (~/.local/share/attune/plugins/<id>/).
    /// 接受 plugin 源目录 — 解析 manifest 拿 id, 复制到目标位置.
    /// 装载前自动校验 (签名 / 加密 / trust↔pricing 联动).
    PluginInstall {
        /// plugin 源目录 (含 plugin.yaml 或 plugin.yaml.enc)
        src: std::path::PathBuf,
        /// paid plugin 的解密密钥 (或 env ATTUNE_PLUGIN_KEY)
        #[arg(long)]
        key: Option<String>,
        /// 签名校验 — 提供 pubkey hex 才校验 plugin.sig
        #[arg(long)]
        pubkey: Option<String>,
        /// 覆盖已装载的同 id plugin
        #[arg(long)]
        force: bool,
    },
    /// 卸载 plugin (从 plugins 目录删除)
    PluginUninstall {
        plugin_id: String,
    },
    /// 列出已装载 plugins (~/.local/share/attune/plugins/)
    PluginList,
    /// 登录云端账号 (走 cloud accounts /api/v1/users/login)
    /// 登录后 settings 大多数项锁定 (云端下发), 并触发 pro 插件自动同步.
    Login {
        email: String,
        /// 云端 accounts base URL (默认 https://accounts.engi-stack.com)
        #[arg(long, default_value = "https://accounts.engi-stack.com")]
        cloud_url: String,
    },
    /// 拉云端 entitled pro 插件清单, 自动下载 + 装载缺的
    SyncPlugins {
        #[arg(long, default_value = "https://accounts.engi-stack.com")]
        cloud_url: String,
    },
    /// 打包 + 上传 plugin 到 pluginhub (开发者侧分发流程)
    /// 流程: 1) tar plugin dir → .attunepkg  2) POST 到 pluginhub /admin/plugins
    PluginPublish {
        /// plugin 源目录 (含 plugin.yaml / bin/ / plugin.sig)
        plugin_dir: std::path::PathBuf,
        /// pluginhub base URL (lawcontrol/pluginhub 部署)
        #[arg(long, default_value = "https://hub.engi-stack.com")]
        hub_url: String,
        /// admin token (env PLUGINHUB_ADMIN_TOKEN)
        #[arg(long)]
        admin_token: Option<String>,
    },
    /// 给当前 vault 关联一个本地知识库目录 (会员/免费用户都可用 — 用户隐私)
    LinkFolder {
        /// 本地目录绝对路径
        folder: std::path::PathBuf,
        /// 关联到的 Project id (默认 default)
        #[arg(long, default_value = "default")]
        project: String,
    },
    /// 列出所有 OCR 场景预设 (builtin + 用户自定义)
    OcrProfileList,
    /// 查看指定 OCR profile 详情 (JSON)
    OcrProfileShow {
        id: String,
    },
    /// 新建 OCR profile (用户自定义, builtin=false)
    OcrProfileCreate {
        /// slug id, e.g. medical-scan
        id: String,
        /// 显示名, e.g. "医学影像"
        #[arg(long)]
        name: String,
        /// 用户可见说明
        #[arg(long, default_value = "")]
        description: String,
        /// 语言代码 (元信息, PP-OCRv5 内置中英)
        #[arg(long, default_value = "chi_sim+eng")]
        languages: String,
        /// PDF 渲染 DPI [72-1200]
        #[arg(long, default_value_t = 300)]
        dpi: u32,
        /// 适用场景标签 (逗号分隔)
        #[arg(long, default_value = "")]
        tags: String,
    },
    /// 删除 OCR profile (builtin 拒绝)
    OcrProfileDelete {
        id: String,
    },
    /// v1.0.1 C4: 列出 / 回滚 vault 备份。
    /// 无 --version 列出 ~/.local/share/Attune/backups/ 内备份(时间 / size / SHA256)。
    /// 带 --version 选最近一份回滚 vault.db(自动备份 current 防双失)。
    Rollback {
        /// 选第 N 份备份(1 = 最新)。不传则列表。
        #[arg(long)]
        index: Option<usize>,
        /// 跳过确认 prompt。
        #[arg(long)]
        yes: bool,
    },
    /// v1.0.1 C4: 强制升级前备份 — `vault.db` → `vault.db.bak.<stamp>`,自动 retention 5 份。
    PreUpgradeBackup,
}

/// CLI exit code protocol (D5.7).
///
/// | code | meaning                                                    |
/// |------|------------------------------------------------------------|
/// | 0    | success                                                    |
/// | 1    | user input error (file missing, unknown profile, bad arg)  |
/// | 2    | reserved — red-line warning (file too big but proceed)     |
/// | 3    | engine / server failure (OCR/ASR model load, transcoding)  |
/// | 4    | network unreachable (reserved for future REST mode)        |
///
/// Most errors today bubble through `VaultError`; this function maps them to
/// integer exit codes. `VaultError::InvalidInput` / `NotFound` (path / profile)
/// → 1; `ModelLoad` / engine subprocess crash → 3; anything else → 1 (default).
fn classify_error_exit_code(err: &attune_core::error::VaultError) -> i32 {
    use attune_core::error::VaultError;
    match err {
        VaultError::InvalidInput(_) | VaultError::NotFound(_) => 1,
        VaultError::ModelLoad(_) => 3,
        VaultError::Io(io_err) => match io_err.kind() {
            std::io::ErrorKind::NotFound | std::io::ErrorKind::InvalidInput => 1,
            _ => 3,
        },
        _ => 1,
    }
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(classify_error_exit_code(&e));
    }
}

fn run(cli: Cli) -> attune_core::error::Result<()> {
    // Plugin pack 子命令组不需要 vault — 早 return
    match &cli.command {
        Commands::PluginEncrypt { plugin_dir, key, delete_plain } => {
            return run_plugin_encrypt(plugin_dir, key.as_deref(), *delete_plain);
        }
        Commands::PluginDecrypt { plugin_dir, key } => {
            return run_plugin_decrypt(plugin_dir, key.as_deref());
        }
        Commands::PluginVerify { plugin_dir, key, trust } => {
            return run_plugin_verify(plugin_dir, key.as_deref(), trust);
        }
        Commands::PluginKeygen { out_priv } => {
            return run_plugin_keygen(out_priv.as_deref());
        }
        Commands::PluginSign { plugin_dir, priv_key, priv_file } => {
            return run_plugin_sign(plugin_dir, priv_key.as_deref(), priv_file.as_deref());
        }
        Commands::PluginVerifySig { plugin_dir, pubkey } => {
            return run_plugin_verify_sig(plugin_dir, pubkey);
        }
        Commands::PluginInstall { src, key, pubkey, force } => {
            return run_plugin_install(src, key.as_deref(), pubkey.as_deref(), *force);
        }
        Commands::PluginUninstall { plugin_id } => {
            return run_plugin_uninstall(plugin_id);
        }
        Commands::PluginList => {
            return run_plugin_list();
        }
        Commands::Login { email, cloud_url } => {
            return run_login(email, cloud_url);
        }
        Commands::SyncPlugins { cloud_url } => {
            return run_sync_plugins(cloud_url);
        }
        Commands::LinkFolder { folder, project } => {
            return run_link_folder(folder, project);
        }
        Commands::PluginPublish { plugin_dir, hub_url, admin_token } => {
            return run_plugin_publish(plugin_dir, hub_url, admin_token.as_deref());
        }
        Commands::OcrProfileList => return run_ocr_profile_list(),
        Commands::OcrProfileShow { id } => return run_ocr_profile_show(id),
        Commands::OcrProfileCreate { id, name, description, languages, dpi, tags } => {
            return run_ocr_profile_create(id, name, description, languages, *dpi, tags);
        }
        Commands::OcrProfileDelete { id } => return run_ocr_profile_delete(id),
        // v1.0.1 C4 — backup/rollback 不需要 vault 已 unlock(直接操作 vault.db 文件)
        Commands::Rollback { index, yes } => return run_rollback(*index, *yes),
        Commands::PreUpgradeBackup => return run_pre_upgrade_backup(),
        // VaultImport must run BEFORE Vault::open_default() — open() auto-creates vault.db
        // via Connection::open(), which would make the "already exists" guard always trigger.
        Commands::VaultImport { src, force } => {
            return run_vault_import(src, *force);
        }
        _ => {}
    }
    // OCR / Deploy 子命令不需要 vault — 早 return 避免 zero-state 报错
    if let Commands::Ocr { image, profile, id_card_subtype, json, bbox } = &cli.command {
        // ── D5.7 pre-validation: friendly errors with structured exit codes ──
        // (1) Profile id must be known (or None for plain mode). Validated FIRST so typo
        //     diagnostics surface before file-not-found (user often types wrong flag, not wrong path).
        validate_ocr_profile_or_suggest(profile.as_deref())?;
        // (2) Image file must exist (exit 1)
        if !image.exists() {
            return Err(attune_core::error::VaultError::InvalidInput(format!(
                "image file not found: {}\n\
                 hint: check the path; cwd is {}",
                image.display(),
                std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "<unknown>".into()),
            )));
        }

        let provider = attune_core::ocr::detect_default_provider().ok_or_else(|| {
            attune_core::error::VaultError::ModelLoad(
                "PP-OCR models missing — run `attune deploy` or apt install --reinstall attune".into(),
            )
        })?;
        eprintln!("[attune ocr] engine: {} | image: {} | profile: {}",
            provider.name(), image.display(), profile.as_deref().unwrap_or("(plain)"));
        let start = std::time::Instant::now();
        let ocr_profile = attune_core::ocr::profile_for_id(profile.as_deref());
        let out = provider.extract_structured(image, &ocr_profile)?;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        eprintln!("[attune ocr] {elapsed_ms}ms elapsed");

        let lines = out.lines.clone().unwrap_or_default();
        let structured = if !lines.is_empty() {
            if let Some(p) = profile.as_deref() {
                attune_core::ocr::structured::extract(p, &lines, id_card_subtype.as_deref())
                    .and_then(|s| serde_json::to_value(s).ok())
            } else {
                None
            }
        } else {
            None
        };

        // D5.7: --no-bbox strips bbox from JSON output (text + confidence only).
        let lines_json: serde_json::Value = if *bbox {
            serde_json::to_value(&lines).unwrap_or(serde_json::Value::Null)
        } else {
            serde_json::Value::Array(
                lines.iter()
                    .map(|l| serde_json::json!({ "text": l.text, "confidence": l.confidence }))
                    .collect(),
            )
        };

        let envelope = serde_json::json!({
            "envelope_version": "1",
            "profile": profile.clone().unwrap_or_else(|| "(plain)".into()),
            "elapsed_ms": elapsed_ms,
            "engine": provider.name(),
            "lines": lines_json,
            "structured": structured,
            "text": out.text,
        });
        if *json {
            println!("{}", serde_json::to_string_pretty(&envelope).unwrap());
        } else {
            println!("{}", out.text);
        }
        return Ok(());
    }
    if let Commands::Transcribe { audio, diarization, json, wait } = &cli.command {
        // ── D5.7 pre-validation: friendly errors with structured exit codes ──
        if !audio.exists() {
            return Err(attune_core::error::VaultError::InvalidInput(format!(
                "audio file not found: {}\n\
                 hint: check the path; cwd is {}",
                audio.display(),
                std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_else(|_| "<unknown>".into()),
            )));
        }
        // `--no-wait` (async mode) is reserved for future REST/daemon path; in-process is always sync.
        if !*wait {
            eprintln!(
                "[attune transcribe] WARN: --no-wait ignored (in-process mode is always synchronous; \
                 flag reserved for future REST daemon mode)"
            );
        }

        let backend = attune_core::asr::detect_asr_backend().ok_or_else(|| {
            attune_core::error::VaultError::ModelLoad(
                "ASR backend missing — whisper-cli not installed or model not downloaded".into(),
            )
        })?;
        eprintln!("[attune transcribe] model: {} | audio: {}",
            backend.model_name, audio.display());
        let diar = if *diarization {
            attune_core::asr::detect_diarization_backend()
        } else {
            None
        };
        let start = std::time::Instant::now();
        let (segments, _legacy_text) =
            attune_core::asr::transcribe_with_diarization(&backend, audio, diar.as_ref())?;
        let elapsed_ms = start.elapsed().as_millis() as u64;
        eprintln!("[attune transcribe] {elapsed_ms}ms elapsed, {} segments", segments.len());
        if *json {
            let value = serde_json::json!({
                "model": backend.model_name,
                "language_detected": backend.language,
                "elapsed_ms": elapsed_ms,
                "diarization_used": diar.is_some(),
                "segments": segments.iter().map(|s| serde_json::json!({
                    "start_sec": s.start_ms as f64 / 1000.0,
                    "end_sec":   s.end_ms   as f64 / 1000.0,
                    "text":      s.text,
                    "speaker":   s.speaker,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&value).unwrap());
        } else {
            for s in &segments {
                println!("{}", s.to_display());
            }
        }
        return Ok(());
    }

    let vault = Vault::open_default()?;

    match cli.command {
        Commands::Setup => {
            let password = read_password("Enter master password: ")?;
            let confirm = read_password("Confirm master password: ")?;
            if password != confirm {
                eprintln!("Passwords do not match.");
                std::process::exit(1);
            }
            let recovery_key = vault.setup_with_recovery_key(&password)?;
            println!("Vault initialized and unlocked.");
            println!("Device secret saved to: {}", attune_core::platform::device_secret_path().display());
            println!("IMPORTANT: Back up your device.key file — you need it to unlock on other devices.");
            println!("Recovery key (store offline, needed for password reset without data loss):");
            println!("{recovery_key}");
        }
        Commands::Unlock => {
            let password = read_password("Enter master password: ")?;
            let token = vault.unlock(&password)?;
            println!("Vault unlocked.");
            println!("Session token: {token}");
        }
        Commands::Lock => {
            vault.lock()?;
            println!("Vault locked. All keys cleared from memory.");
        }
        Commands::Status => {
            let state = vault.state();
            let count = if matches!(state, attune_core::vault::VaultState::Unlocked) {
                vault.store().item_count().unwrap_or(0)
            } else {
                0
            };
            let status = serde_json::json!({
                "state": state,
                "items": count,
                "data_dir": attune_core::platform::data_dir(),
                "config_dir": attune_core::platform::config_dir(),
            });
            println!("{}", serde_json::to_string_pretty(&status).expect("status JSON object is serializable"));
        }
        Commands::Insert { title, content, source_type } => {
            let dek = vault.dek_db()?;
            let id = vault.store().insert_item(&dek, &title, &content, None, &source_type, None, None)?;
            println!("Inserted: {id}");
        }
        Commands::Get { id } => {
            let dek = vault.dek_db()?;
            match vault.store().get_item(&dek, &id)? {
                Some(item) => println!("{}", serde_json::to_string_pretty(&item).expect("Item is serializable")),
                None => {
                    eprintln!("Item not found: {id}");
                    std::process::exit(1);
                }
            }
        }
        Commands::List { limit } => {
            let _ = vault.dek_db()?;
            let items = vault.store().list_items(limit, 0)?;
            println!("{}", serde_json::to_string_pretty(&items).expect("Vec<Item> is serializable"));
        }
        Commands::VaultExport { dest } => {
            // R38: 必须 locked，否则 SQLite WAL 文件可能不一致
            if matches!(vault.state(), attune_core::vault::VaultState::Unlocked) {
                eprintln!("Refusing to export while vault is UNLOCKED — please run `attune lock` first.");
                eprintln!("Reason: SQLite WAL must be checkpointed; locking forces a consistent snapshot.");
                std::process::exit(1);
            }
            let data = attune_core::platform::data_dir();
            std::fs::create_dir_all(&dest).map_err(attune_core::error::VaultError::Io)?;
            let mut copied = 0u32;
            for name in &["vault.db", "vault.db-shm", "vault.db-wal", "vectors.encbin"] {
                let src = data.join(name);
                if src.exists() {
                    let target = dest.join(name);
                    std::fs::copy(&src, &target).map_err(attune_core::error::VaultError::Io)?;
                    copied += 1;
                }
            }
            // tantivy directory recursive copy
            let ftx_src = data.join("tantivy");
            if ftx_src.is_dir() {
                let ftx_dst = dest.join("tantivy");
                copy_dir_recursive(&ftx_src, &ftx_dst).map_err(attune_core::error::VaultError::Io)?;
                copied += 1;
            }
            println!("Exported {copied} entries to {}", dest.display());
            println!("IMPORTANT: separately back up your device.key at {}",
                attune_core::platform::device_secret_path().display());
        }
        Commands::Ocr { .. } => unreachable!("Ocr handled before vault open"),
        Commands::Transcribe { .. } => unreachable!("Transcribe handled before vault open"),
        Commands::Deploy { no_models, dry_run, script } => {
            // R-deploy: 调底层 bash 脚本。Linux-only。
            if !cfg!(target_os = "linux") {
                eprintln!("attune deploy 当前仅支持 Linux（当前平台 = {}）。", std::env::consts::OS);
                eprintln!("Windows: 用 MSI 安装包；macOS: 暂不支持。");
                std::process::exit(2);
            }
            let script_path = script.unwrap_or_else(|| {
                std::path::PathBuf::from("scripts/deploy-linux.sh")
            });
            if !script_path.exists() {
                eprintln!("deploy script 不存在: {}", script_path.display());
                eprintln!("请从源码仓库根目录运行 `attune deploy`，或用 --script <path> 指定。");
                std::process::exit(2);
            }
            let mut cmd = std::process::Command::new("bash");
            cmd.arg(&script_path);
            if no_models { cmd.arg("--no-models"); }
            if dry_run { cmd.arg("--dry-run"); }
            let status = cmd.status().map_err(attune_core::error::VaultError::Io)?;
            if !status.success() {
                std::process::exit(status.code().unwrap_or(1));
            }
            // 部署后给一条提示让用户启动 attune-server-headless
            println!();
            println!("✓ deploy 完成。下一步：");
            println!("  1. 初始化 vault:        attune setup");
            println!("  2. 启动 server:         attune-server-headless --port 18900");
            println!("  3. 浏览器访问:          http://localhost:18900");
        }
        Commands::VaultImport { .. } => {
            unreachable!("VaultImport handled before vault open")
        }
        // Plugin / cloud / OCR profile 子命令在 run() 头部已 handle, 这里 unreachable
        Commands::PluginEncrypt { .. } | Commands::PluginDecrypt { .. } | Commands::PluginVerify { .. }
        | Commands::PluginKeygen { .. } | Commands::PluginSign { .. } | Commands::PluginVerifySig { .. }
        | Commands::PluginInstall { .. } | Commands::PluginUninstall { .. } | Commands::PluginList
        | Commands::Login { .. } | Commands::SyncPlugins { .. } | Commands::LinkFolder { .. }
        | Commands::PluginPublish { .. }
        | Commands::OcrProfileList | Commands::OcrProfileShow { .. }
        | Commands::OcrProfileCreate { .. } | Commands::OcrProfileDelete { .. }
        | Commands::Rollback { .. } | Commands::PreUpgradeBackup => {
            unreachable!("plugin/cloud/ocr-profile commands handled before vault open")
        }
    }
    Ok(())
}

/// D5.7: list of valid OCR scene profile ids (mirrors `structured::extract` match arms).
const VALID_OCR_PROFILES: &[&str] = &["document", "receipt", "table", "card", "id_card"];

/// D5.7: validate `--profile <id>` against known set; on typo, suggest nearest match.
///
/// Returns:
///   - `Ok(())` if `profile_id` is `None` (plain mode) or matches a known scene.
///   - `Err(VaultError::InvalidInput)` with suggestion if typo, or with full list if no close match.
fn validate_ocr_profile_or_suggest(profile_id: Option<&str>) -> attune_core::error::Result<()> {
    let id = match profile_id {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(()), // None or "" → plain mode, no structured extraction
    };
    if VALID_OCR_PROFILES.contains(&id) {
        return Ok(());
    }
    // Find nearest match (levenshtein ≤ 2 considered close)
    let suggestion = VALID_OCR_PROFILES
        .iter()
        .map(|valid| (*valid, strsim::levenshtein(id, valid)))
        .min_by_key(|(_, dist)| *dist);
    let hint = match suggestion {
        Some((best, dist)) if dist <= 2 => format!(
            "unknown profile: '{id}'\n\
             hint: did you mean '{best}'? (edit distance {dist})\n\
             valid profiles: {}",
            VALID_OCR_PROFILES.join(", ")
        ),
        _ => format!(
            "unknown profile: '{id}'\n\
             valid profiles: {}",
            VALID_OCR_PROFILES.join(", ")
        ),
    };
    Err(attune_core::error::VaultError::InvalidInput(hint))
}

/// 递归复制目录 — 用于 vault export/import 的 tantivy/ 子目录
fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let ft = entry.file_type()?;
        let dst_path = dst.join(entry.file_name());
        if ft.is_dir() {
            copy_dir_recursive(&entry.path(), &dst_path)?;
        } else if ft.is_file() {
            std::fs::copy(entry.path(), dst_path)?;
        }
    }
    Ok(())
}

fn read_password(prompt: &str) -> attune_core::error::Result<String> {
    eprint!("{prompt}");
    rpassword::read_password().map_err(attune_core::error::VaultError::Io)
}


fn read_plugin_key(arg_key: Option<&str>) -> attune_core::error::Result<Vec<u8>> {
    if let Some(k) = arg_key {
        return Ok(k.as_bytes().to_vec());
    }
    if let Ok(k) = std::env::var("ATTUNE_PLUGIN_KEY") {
        return Ok(k.into_bytes());
    }
    Err(attune_core::error::VaultError::InvalidInput(
        "plugin key required (--key or env ATTUNE_PLUGIN_KEY)".into(),
    ))
}

fn run_plugin_encrypt(
    plugin_dir: &std::path::Path,
    key: Option<&str>,
    delete_plain: bool,
) -> attune_core::error::Result<()> {
    let key = read_plugin_key(key)?;
    let yaml_path = plugin_dir.join("plugin.yaml");
    let enc_path = plugin_dir.join("plugin.yaml.enc");
    if !yaml_path.exists() {
        return Err(attune_core::error::VaultError::InvalidInput(format!(
            "plugin.yaml not found at {}",
            yaml_path.display()
        )));
    }
    let plain = std::fs::read(&yaml_path).map_err(attune_core::error::VaultError::Io)?;
    let cipher = attune_core::plugin_encryption::encrypt_yaml(&plain, &key)?;
    std::fs::write(&enc_path, &cipher).map_err(attune_core::error::VaultError::Io)?;
    eprintln!("✓ encrypted to {} ({} bytes)", enc_path.display(), cipher.len());
    if delete_plain {
        std::fs::remove_file(&yaml_path).map_err(attune_core::error::VaultError::Io)?;
        eprintln!("✓ removed plaintext plugin.yaml");
    }
    Ok(())
}

fn run_plugin_decrypt(
    plugin_dir: &std::path::Path,
    key: Option<&str>,
) -> attune_core::error::Result<()> {
    let key = read_plugin_key(key)?;
    let enc_path = plugin_dir.join("plugin.yaml.enc");
    let yaml_path = plugin_dir.join("plugin.yaml");
    if !enc_path.exists() {
        return Err(attune_core::error::VaultError::InvalidInput(format!(
            "plugin.yaml.enc not found at {}",
            enc_path.display()
        )));
    }
    let cipher = std::fs::read(&enc_path).map_err(attune_core::error::VaultError::Io)?;
    let plain = attune_core::plugin_encryption::decrypt_yaml(&cipher, &key)?;
    std::fs::write(&yaml_path, &plain).map_err(attune_core::error::VaultError::Io)?;
    eprintln!("✓ decrypted to {} ({} bytes)", yaml_path.display(), plain.len());
    Ok(())
}

fn run_plugin_verify(
    plugin_dir: &std::path::Path,
    key: Option<&str>,
    trust: &str,
) -> attune_core::error::Result<()> {
    let key_bytes: Option<Vec<u8>> = if plugin_dir.join("plugin.yaml.enc").exists() {
        Some(read_plugin_key(key)?)
    } else {
        None
    };
    let plugin = attune_core::plugin_loader::LoadedPlugin::from_dir_with_key(
        plugin_dir,
        key_bytes.as_deref(),
        Some(trust),
    )?;
    eprintln!("✓ plugin loaded: id={}, version={}, type={}",
        plugin.manifest.id, plugin.manifest.version, plugin.manifest.plugin_type);
    if let Some(p) = &plugin.manifest.pricing {
        eprintln!("  pricing: tier={}", p.tier);
    }
    eprintln!("  skills: {}", plugin.manifest.skills.len());
    eprintln!("  agents: {}", plugin.manifest.agents.len());
    eprintln!("  mcp_servers: {}", plugin.manifest.mcp_servers.len());
    eprintln!("  case_kinds: {}", plugin.manifest.registers_case_kinds.len());
    eprintln!("  trust verified: {trust}");
    Ok(())
}

fn run_plugin_keygen(out_priv: Option<&std::path::Path>) -> attune_core::error::Result<()> {
    let sk = attune_core::plugin_sig::generate_signing_key();
    let pk_hex = attune_core::plugin_sig::derive_verifying_key_hex(&sk);
    let sk_hex = hex::encode(sk);

    if let Some(path) = out_priv {
        std::fs::write(path, &sk_hex).map_err(attune_core::error::VaultError::Io)?;
        // 限制权限 600 (Unix)
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600));
        }
        eprintln!("✓ private key written to {} (chmod 600 on Unix)", path.display());
    } else {
        println!("PRIVATE_KEY={sk_hex}");
        eprintln!("⚠️  Private key printed to stdout — save it offline immediately and clear shell history.");
    }
    println!("PUBLIC_KEY={pk_hex}");
    eprintln!("Public key (embed in OFFICIAL_PUBLIC_KEYS or distribute): {pk_hex}");
    Ok(())
}

fn read_signing_key(
    priv_key_arg: Option<&str>,
    priv_file_arg: Option<&std::path::Path>,
) -> attune_core::error::Result<[u8; 32]> {
    let hex_str = if let Some(k) = priv_key_arg {
        k.to_string()
    } else if let Some(p) = priv_file_arg {
        std::fs::read_to_string(p)
            .map_err(attune_core::error::VaultError::Io)?
            .trim()
            .to_string()
    } else if let Ok(k) = std::env::var("ATTUNE_PLUGIN_SIGN_KEY") {
        k
    } else {
        return Err(attune_core::error::VaultError::InvalidInput(
            "signing key required (--priv-key / --priv-file / env ATTUNE_PLUGIN_SIGN_KEY)".into(),
        ));
    };
    let bytes = hex::decode(hex_str.trim())
        .map_err(|e| attune_core::error::VaultError::InvalidInput(format!("bad hex: {e}")))?;
    bytes
        .as_slice()
        .try_into()
        .map_err(|_| attune_core::error::VaultError::InvalidInput("private key must be 32 bytes".into()))
}

fn run_plugin_sign(
    plugin_dir: &std::path::Path,
    priv_key: Option<&str>,
    priv_file: Option<&std::path::Path>,
) -> attune_core::error::Result<()> {
    let sk = read_signing_key(priv_key, priv_file)?;
    let sig = attune_core::plugin_sig::sign_plugin(plugin_dir, &sk)?;
    eprintln!("✓ plugin.sig written to {}", plugin_dir.join("plugin.sig").display());
    eprintln!("  signature (base64): {sig}");
    Ok(())
}

fn run_plugin_verify_sig(
    plugin_dir: &std::path::Path,
    pubkey: &str,
) -> attune_core::error::Result<()> {
    let ok = attune_core::plugin_sig::verify_with_key(plugin_dir, pubkey)?;
    if ok {
        eprintln!("✓ signature VALID");
        Ok(())
    } else {
        eprintln!("✗ signature INVALID");
        std::process::exit(1);
    }
}

fn run_plugin_install(
    src: &std::path::Path,
    key: Option<&str>,
    pubkey: Option<&str>,
    force: bool,
) -> attune_core::error::Result<()> {
    // 1. 签名校验先行 (用于推导 trust 级别, paid plugin 装载校验需要)
    let trust = if let Some(pk) = pubkey {
        let ok = attune_core::plugin_sig::verify_with_key(src, pk)?;
        if !ok {
            return Err(attune_core::error::VaultError::InvalidInput(
                "plugin.sig verification FAILED".into(),
            ));
        }
        eprintln!("✓ signature verified with provided pubkey → trust=Trusted");
        "Trusted"
    } else {
        eprintln!("⚠️  no --pubkey: trust=Unsigned (paid plugin will be rejected)");
        "Unsigned"
    };

    // 2. 解析 src plugin.yaml 拿 id (paid plugin 需提供 key + 合格 trust)
    let key_bytes: Option<Vec<u8>> = if src.join("plugin.yaml.enc").exists() {
        Some(read_plugin_key(key)?)
    } else {
        None
    };
    let plugin = attune_core::plugin_loader::LoadedPlugin::from_dir_with_key(
        src,
        key_bytes.as_deref(),
        Some(trust),
    )?;
    let plugin_id = plugin.manifest.id.clone();
    eprintln!("✓ parsed plugin: id={plugin_id}, version={}", plugin.manifest.version);

    // 3. 解析目标安装目录
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()?;
    std::fs::create_dir_all(&plugins_root).map_err(attune_core::error::VaultError::Io)?;
    let dst = plugins_root.join(&plugin_id);

    // 4. 检查冲突
    if dst.exists() {
        if !force {
            return Err(attune_core::error::VaultError::InvalidInput(format!(
                "plugin '{plugin_id}' already installed at {} (use --force to overwrite)",
                dst.display()
            )));
        }
        eprintln!("⚠️  removing existing {} (--force)", dst.display());
        std::fs::remove_dir_all(&dst).map_err(attune_core::error::VaultError::Io)?;
    }

    // 5. 复制源目录到目标
    copy_dir_recursive(src, &dst)?;
    eprintln!("✓ installed to {}", dst.display());
    eprintln!("Restart attune-server for the new plugin to be loaded.");
    Ok(())
}

fn run_plugin_uninstall(plugin_id: &str) -> attune_core::error::Result<()> {
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()?;
    let dst = plugins_root.join(plugin_id);
    if !dst.exists() {
        return Err(attune_core::error::VaultError::InvalidInput(format!(
            "plugin '{plugin_id}' not installed at {}",
            dst.display()
        )));
    }
    std::fs::remove_dir_all(&dst).map_err(attune_core::error::VaultError::Io)?;
    eprintln!("✓ uninstalled {plugin_id}");
    Ok(())
}

fn run_plugin_list() -> attune_core::error::Result<()> {
    let plugins_root = attune_core::plugin_registry::PluginRegistry::default_plugins_dir()?;
    if !plugins_root.exists() {
        println!("No plugins installed (dir does not exist: {})", plugins_root.display());
        return Ok(());
    }
    let mut count = 0usize;
    let mut errors = 0usize;
    for entry in std::fs::read_dir(&plugins_root).map_err(attune_core::error::VaultError::Io)? {
        let entry = entry.map_err(attune_core::error::VaultError::Io)?;
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }
        // list 是诊断命令, 不强制 trust 校验 (绕开 paid+Unsigned 联动). 真实装载时
        // attune-server scan 仍会按 trust 拒绝.
        match attune_core::plugin_loader::LoadedPlugin::from_dir_with_key(&path, None, Some("Official")) {
            Ok(plugin) => {
                count += 1;
                let m = &plugin.manifest;
                let tier = m.pricing.as_ref().map(|p| p.tier.as_str()).unwrap_or("?");
                println!(
                    "  {} (v{}, type={}, tier={}, agents={}, skills={}, mcps={})",
                    m.id, m.version, m.plugin_type, tier,
                    m.agents.len(), m.skills.len(), m.mcp_servers.len()
                );
            }
            Err(e) => {
                errors += 1;
                eprintln!("  [error] {}: {e}", path.display());
            }
        }
    }
    println!("{count} plugin(s) installed at {} ({errors} errors)", plugins_root.display());
    Ok(())
}

fn run_login(email: &str, cloud_url: &str) -> attune_core::error::Result<()> {
    let password = read_password(&format!("Password for {email}: "))?;
    let mut client = attune_core::cloud_client::CloudClient::new(cloud_url);
    let user = client.login(email, &password)?;
    eprintln!("✓ logged in as {} (plan={})", user.email, user.plan);

    // 持久化 session token，供后续 sync-plugins 等跨进程调用使用
    if let Some(token) = client.session_token() {
        persist_cloud_session(cloud_url, token)?;
    }

    // 拿 licenses + entitled plugins, 提示是否自动同步
    match client.list_licenses() {
        Ok(licenses) => {
            eprintln!("  你有 {} 个 license:", licenses.len());
            for lic in &licenses {
                let name_str = lic.name.as_deref().unwrap_or("-");
                eprintln!(
                    "  - id={} name={} plan={} plugins={}",
                    lic.id, name_str, lic.plan, lic.entitled_plugins.len()
                );
                if !lic.entitled_plugins.is_empty() {
                    eprintln!("    entitled plugins:");
                    for ep in &lic.entitled_plugins {
                        eprintln!("    · {} (v{})", ep.plugin_id, ep.version);
                    }
                }
            }
            eprintln!();
            eprintln!("运行 `attune sync-plugins` 自动装 entitled pro 插件");

            // accounts 下发的 license_key 是 Bearer token, 不是 SignedLicense code.
            // 客户端登录目的: 鉴权 + session 持久化; paid plugin 解密 key 走 plugin_sync
            // 从 EntitledPlugin.decrypt_key 字段直拿, 不需要本地 cache.
            eprintln!("  (info: cloud accounts uses bearer tokens — no local license cache needed)");
        }
        Err(e) => eprintln!("⚠️  list licenses failed: {e}"),
    }
    Ok(())
}

/// 云端 session 持久化文件格式
#[derive(serde::Serialize, serde::Deserialize)]
struct CloudSession {
    cloud_url: String,
    /// accounts 服务返回的 session cookie 值 (完整 "session=<token>" 或裸 token)
    session: String,
}

/// 把 session token 写到 config_dir/cloud-session.json (chmod 600 on Unix)
fn persist_cloud_session(cloud_url: &str, session_token: &str) -> attune_core::error::Result<()> {
    use attune_core::error::VaultError;
    let path = attune_core::platform::config_dir().join("cloud-session.json");
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(VaultError::Io)?;
    }
    let data = CloudSession {
        cloud_url: cloud_url.to_string(),
        session: session_token.to_string(),
    };
    let json = serde_json::to_string_pretty(&data)
        .map_err(|e| VaultError::Crypto(format!("session ser: {e}")))?;
    std::fs::write(&path, &json).map_err(VaultError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
    }
    eprintln!("  ✓ session persisted to {}", path.display());
    Ok(())
}

/// 从 config_dir/cloud-session.json 读回 session, 构造已鉴权的 CloudClient
fn load_cloud_client_with_session(cloud_url: &str) -> attune_core::error::Result<attune_core::cloud_client::CloudClient> {
    use attune_core::error::VaultError;
    let path = attune_core::platform::config_dir().join("cloud-session.json");
    if !path.exists() {
        return Err(VaultError::Crypto(
            "no cloud session found — run `attune login` first".into(),
        ));
    }
    let json = std::fs::read_to_string(&path).map_err(VaultError::Io)?;
    let sess: CloudSession = serde_json::from_str(&json)
        .map_err(|e| VaultError::Crypto(format!("cloud session parse: {e}")))?;
    // cloud_url 参数优先 (CLI flag); 文件里的 url 作为 fallback
    let effective_url = if !cloud_url.is_empty() { cloud_url } else { &sess.cloud_url };
    Ok(attune_core::cloud_client::CloudClient::with_session(
        effective_url,
        &sess.session,
    ))
}

fn run_sync_plugins(cloud_url: &str) -> attune_core::error::Result<()> {
    let client = load_cloud_client_with_session(cloud_url)?;
    let report = attune_core::plugin_sync::sync_plugins(&client)?;
    eprintln!("=== plugin sync report ===");
    eprintln!("  ✓ installed: {}", report.installed.len());
    for p in &report.installed {
        eprintln!("    + {p}");
    }
    eprintln!("  · skipped (already installed): {}", report.skipped_already_installed.len());
    for p in &report.skipped_already_installed {
        eprintln!("    = {p}");
    }
    if !report.failed.is_empty() {
        eprintln!("  ❌ failed: {}", report.failed.len());
        for (p, reason) in &report.failed {
            eprintln!("    ✗ {p}: {reason}");
        }
    }
    eprintln!();
    eprintln!("Restart attune-server for newly installed plugins to be picked up.");
    Ok(())
}

fn run_link_folder(folder: &std::path::Path, project: &str) -> attune_core::error::Result<()> {
    if !folder.exists() {
        return Err(attune_core::error::VaultError::InvalidInput(format!(
            "folder does not exist: {}",
            folder.display()
        )));
    }
    if !folder.is_dir() {
        return Err(attune_core::error::VaultError::InvalidInput(format!(
            "not a directory: {}",
            folder.display()
        )));
    }
    let abs = std::fs::canonicalize(folder).map_err(attune_core::error::VaultError::Io)?;

    // 把 link 写到 ~/.config/attune/folder-links.json (UI/server 启动时读)
    let config_dir = attune_core::platform::config_dir();
    std::fs::create_dir_all(&config_dir).map_err(attune_core::error::VaultError::Io)?;
    let links_path = config_dir.join("folder-links.json");
    let mut links: Vec<FolderLink> = if links_path.exists() {
        let s = std::fs::read_to_string(&links_path).map_err(attune_core::error::VaultError::Io)?;
        serde_json::from_str(&s).unwrap_or_default()
    } else {
        Vec::new()
    };
    let new_link = FolderLink {
        project: project.to_string(),
        folder: abs.to_string_lossy().to_string(),
        linked_at: chrono::Utc::now().to_rfc3339(),
    };
    // 去重 (按 folder)
    links.retain(|l| l.folder != new_link.folder);
    links.push(new_link.clone());
    std::fs::write(
        &links_path,
        serde_json::to_string_pretty(&links).expect("ser"),
    ).map_err(attune_core::error::VaultError::Io)?;

    eprintln!("✓ linked {} to project '{}'", abs.display(), project);
    eprintln!("  link saved to {}", links_path.display());
    eprintln!("  total links: {}", links.len());
    Ok(())
}

#[derive(serde::Serialize, serde::Deserialize, Clone)]
struct FolderLink {
    project: String,
    folder: String,
    linked_at: String,
}

fn run_plugin_publish(
    plugin_dir: &std::path::Path,
    hub_url: &str,
    admin_token_arg: Option<&str>,
) -> attune_core::error::Result<()> {
    let admin_token = admin_token_arg
        .map(String::from)
        .or_else(|| std::env::var("PLUGINHUB_ADMIN_TOKEN").ok())
        .ok_or_else(|| attune_core::error::VaultError::InvalidInput(
            "admin token required (--admin-token or env PLUGINHUB_ADMIN_TOKEN)".into(),
        ))?;

    // 1. 解析 manifest 拿 id + version
    let plugin = attune_core::plugin_loader::LoadedPlugin::from_dir_with_key(
        plugin_dir, None, Some("Trusted"),
    )?;
    let id = plugin.manifest.id.clone();
    let version = plugin.manifest.version.clone();
    eprintln!("✓ plugin: id={id}, version={version}");

    // 2. tar plugin dir → .attunepkg (临时文件)
    let tmp = tempfile::tempdir().map_err(attune_core::error::VaultError::Io)?;
    let pkg_path = tmp.path().join(format!("{id}-{version}.attunepkg"));
    let status = std::process::Command::new("tar")
        .args(["czf"])
        .arg(&pkg_path)
        .args(["-C", plugin_dir.parent().unwrap_or(std::path::Path::new(".")).to_string_lossy().as_ref()])
        .arg(plugin_dir.file_name().unwrap_or_default())
        .status()
        .map_err(attune_core::error::VaultError::Io)?;
    if !status.success() {
        return Err(attune_core::error::VaultError::Io(std::io::Error::other(format!(
            "tar exit {:?}", status.code()
        ))));
    }
    let size = std::fs::metadata(&pkg_path).map(|m| m.len()).unwrap_or(0);
    eprintln!("✓ packaged: {} ({} bytes)", pkg_path.display(), size);

    // 3a. 创建插件元信息 — POST /api/v1/admin/plugins/ (trailing slash, FastAPI 无重定向)
    // 409 表示插件已存在，不阻止继续上传新版本。
    let base = hub_url.trim_end_matches('/');
    let client = reqwest::blocking::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| attune_core::error::VaultError::Io(std::io::Error::other(format!("http client: {e}"))))?;

    let meta_url = format!("{base}/api/v1/admin/plugins/");
    let category = if plugin.manifest.category.is_empty() { "general" } else { &plugin.manifest.category };
    let meta_form = reqwest::blocking::multipart::Form::new()
        .text("id", id.clone())
        .text("name", plugin.manifest.name.clone())
        .text("type", plugin.manifest.plugin_type.clone())
        .text("category", category.to_string())
        .text("description", plugin.manifest.description.clone());
    eprintln!("→ POST {meta_url}  (create metadata)");
    let meta_resp = client
        .post(&meta_url)
        .header("Authorization", format!("Bearer {admin_token}"))
        .multipart(meta_form)
        .send()
        .map_err(|e| attune_core::error::VaultError::Io(std::io::Error::other(format!("metadata: {e}"))))?;
    let meta_status = meta_resp.status();
    let meta_body = meta_resp.text().unwrap_or_default();
    if meta_status == reqwest::StatusCode::CONFLICT {
        eprintln!("  plugin already exists (409), skipping metadata creation");
    } else if !meta_status.is_success() {
        return Err(attune_core::error::VaultError::Io(std::io::Error::other(format!(
            "metadata failed: {meta_status} body={meta_body}"
        ))));
    } else {
        eprintln!("✓ metadata created: {meta_body}");
    }

    // 3b. 上传版本包 — POST /api/v1/admin/plugins/{id}/versions
    let ver_url = format!("{base}/api/v1/admin/plugins/{id}/versions");
    let bytes = std::fs::read(&pkg_path).map_err(attune_core::error::VaultError::Io)?;
    let ver_form = reqwest::blocking::multipart::Form::new()
        .part(
            "file",
            reqwest::blocking::multipart::Part::bytes(bytes)
                .file_name(format!("{id}-{version}.attunepkg"))
                .mime_str("application/octet-stream")
                .unwrap(),
        )
        .text("changelog", "")
        .text("min_core_version", "0.4.0");
    eprintln!("→ POST {ver_url}  (upload version)");
    let ver_resp = client
        .post(&ver_url)
        .header("Authorization", format!("Bearer {admin_token}"))
        .multipart(ver_form)
        .send()
        .map_err(|e| attune_core::error::VaultError::Io(std::io::Error::other(format!("upload: {e}"))))?;
    let ver_status = ver_resp.status();
    let ver_body = ver_resp.text().unwrap_or_default();
    if !ver_status.is_success() {
        return Err(attune_core::error::VaultError::Io(std::io::Error::other(format!(
            "publish failed: {ver_status} body={ver_body}"
        ))));
    }
    eprintln!("✓ published {id}@{version}: {ver_body}");
    Ok(())
}

// ============ OCR Profile 子命令 ============
// 直接操作本地 <data_dir>/ocr_profiles.json — 不依赖 attune-server 运行,
// vault 锁定状态也能用.

fn run_ocr_profile_list() -> attune_core::error::Result<()> {
    let reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()?;
    println!("{:<14} {:<6} {:<5} {:<14} name", "id", "type", "dpi", "tags");
    println!("{}", "-".repeat(70));
    for p in reg.list() {
        let t = if p.builtin { "builtin" } else { "custom" };
        let tags = p.tags.join(",");
        // 中文 UTF-8 边界安全: 按 char 截断 (避免字节中切)
        let tags_short: String = tags.chars().take(14).collect();
        println!("{:<14} {:<6} {:<5} {:<14} {}", p.id, t, p.dpi, tags_short, p.name);
    }
    Ok(())
}

fn run_ocr_profile_show(id: &str) -> attune_core::error::Result<()> {
    let reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()?;
    match reg.get(id) {
        Some(p) => {
            let body = serde_json::to_string_pretty(p)
                .map_err(|e| attune_core::error::VaultError::InvalidInput(e.to_string()))?;
            println!("{body}");
            Ok(())
        }
        None => Err(attune_core::error::VaultError::NotFound(format!("profile {id}"))),
    }
}

#[allow(clippy::too_many_arguments)]
fn run_ocr_profile_create(
    id: &str,
    name: &str,
    description: &str,
    languages: &str,
    dpi: u32,
    tags_csv: &str,
) -> attune_core::error::Result<()> {
    let tags: Vec<String> = tags_csv
        .split(',')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .collect();
    let p = attune_core::ocr::profile::OcrProfile {
        id: id.to_string(),
        name: name.to_string(),
        description: description.to_string(),
        languages: languages.to_string(),
        dpi,
        tags,
        builtin: false,
        deskew: false,
        reconstruct_tables: false,
        max_side_len: attune_core::ocr::profile::OcrProfile::DEFAULT_MAX_SIDE_LEN,
    };
    let mut reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()?;
    reg.upsert(p)?;
    eprintln!("✓ profile {id} 已写入 {}", attune_core::ocr::profile_registry::ProfileRegistry::default_path().display());
    Ok(())
}

fn run_ocr_profile_delete(id: &str) -> attune_core::error::Result<()> {
    let mut reg = attune_core::ocr::profile_registry::ProfileRegistry::load_default()?;
    reg.delete(id)?;
    eprintln!("✓ profile {id} 已删除");
    Ok(())
}

/// v1.0.1 C4: `attune pre-upgrade-backup` — 强制升级前备份 + retention 5。
fn run_pre_upgrade_backup() -> attune_core::error::Result<()> {
    let entry = attune_core::backup::create_pre_upgrade_backup()?;
    eprintln!("✓ backup 已创建: {}", entry.path.display());
    eprintln!("  size: {} bytes", entry.size);
    eprintln!("  stamp: {}", entry.stamp);
    Ok(())
}

/// v1.0.1 C4: `attune rollback [--index N]` — 列出 / 回滚 vault 备份。
fn run_rollback(index: Option<usize>, yes: bool) -> attune_core::error::Result<()> {
    let entries = attune_core::backup::list_backups()?;
    if entries.is_empty() {
        eprintln!("没有可用 backup(目录: {})", attune_core::backup::backup_dir()?.display());
        eprintln!("提示:升级前先跑 `attune pre-upgrade-backup` 创建备份。");
        return Ok(());
    }
    match index {
        None => {
            // 列表模式
            eprintln!("可用 backup ({} 份,1 = 最新):", entries.len());
            for (i, e) in entries.iter().enumerate() {
                eprintln!("  [{}] {} — {} bytes", i + 1, e.filename, e.size);
            }
            eprintln!();
            eprintln!("回滚:`attune rollback --index <N>`");
        }
        Some(n) => {
            // 回滚模式
            if !yes {
                eprintln!("⚠️  即将回滚 vault.db 到 backup #{}(自动备份当前 vault 防双失)。", n);
                eprintln!("    继续? 输入 'yes' 确认:");
                let mut input = String::new();
                std::io::stdin().read_line(&mut input)
                    .map_err(attune_core::error::VaultError::Io)?;
                if input.trim() != "yes" {
                    eprintln!("中止。");
                    return Ok(());
                }
            }
            let restored = attune_core::backup::restore_from_index(n)?;
            eprintln!("✓ vault.db 已回滚到: {}", restored.filename);
            eprintln!("  原 vault.db 备份为 vault.db.before-rollback.<ts>(防双失)");
        }
    }
    Ok(())
}

/// `attune vault-import <src>` — runs BEFORE Vault::open_default() so that
/// Connection::open() never auto-creates an empty vault.db that would make
/// the "already exists" guard always fire (#61).
fn run_vault_import(src: &std::path::Path, force: bool) -> attune_core::error::Result<()> {
    let data = attune_core::platform::data_dir();
    let target_db = data.join("vault.db");
    if target_db.exists() && !force {
        eprintln!("Refusing to import — {} already exists.", target_db.display());
        eprintln!("Use --force to overwrite (DANGEROUS, replaces current vault).");
        std::process::exit(1);
    }
    if !src.is_dir() {
        eprintln!("Source not a directory: {}", src.display());
        std::process::exit(1);
    }
    let src_db = src.join("vault.db");
    if !src_db.exists() {
        eprintln!("Source missing vault.db: {}", src_db.display());
        std::process::exit(1);
    }
    std::fs::create_dir_all(&data).map_err(attune_core::error::VaultError::Io)?;
    let mut copied = 0u32;
    for name in &["vault.db", "vault.db-shm", "vault.db-wal", "vectors.encbin"] {
        let s = src.join(name);
        if s.exists() {
            std::fs::copy(&s, data.join(name)).map_err(attune_core::error::VaultError::Io)?;
            copied += 1;
        }
    }
    let ftx_src = src.join("tantivy");
    if ftx_src.is_dir() {
        let ftx_dst = data.join("tantivy");
        let _ = std::fs::remove_dir_all(&ftx_dst);
        copy_dir_recursive(&ftx_src, &ftx_dst).map_err(attune_core::error::VaultError::Io)?;
        copied += 1;
    }
    println!("Imported {copied} entries from {}", src.display());
    println!("Run `attune unlock` to verify with the matching master password.");
    println!("If unlock fails, ensure device.key matches: {}",
        attune_core::platform::device_secret_path().display());
    Ok(())
}

#[cfg(test)]
mod cli_helpers_tests {
    use super::*;

    // ── D5.7 validate_ocr_profile_or_suggest ────────────────────────────

    #[test]
    fn validate_profile_accepts_known() {
        for valid in VALID_OCR_PROFILES {
            assert!(
                validate_ocr_profile_or_suggest(Some(valid)).is_ok(),
                "expected {valid} accepted"
            );
        }
    }

    #[test]
    fn validate_profile_accepts_none() {
        assert!(validate_ocr_profile_or_suggest(None).is_ok());
    }

    #[test]
    fn validate_profile_accepts_empty_string_as_plain() {
        assert!(validate_ocr_profile_or_suggest(Some("")).is_ok());
    }

    #[test]
    fn validate_profile_suggests_nearest_on_typo() {
        let err = validate_ocr_profile_or_suggest(Some("recipt")).unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("recipt"), "msg should echo bad input: {msg}");
        assert!(msg.contains("receipt"), "msg should suggest receipt: {msg}");
    }

    #[test]
    fn validate_profile_lists_valid_set_when_no_close_match() {
        let err = validate_ocr_profile_or_suggest(Some("zzzzzzzzzzzz")).unwrap_err();
        let msg = err.to_string();
        // No suggestion (edit distance too large); just lists valid set.
        assert!(msg.contains("valid profiles"), "should list valid set: {msg}");
        assert!(msg.contains("document"));
    }

    // ── D5.7 classify_error_exit_code ──────────────────────────────────

    #[test]
    fn exit_code_invalid_input_maps_to_1() {
        let err = attune_core::error::VaultError::InvalidInput("bad".into());
        assert_eq!(classify_error_exit_code(&err), 1);
    }

    #[test]
    fn exit_code_not_found_maps_to_1() {
        let err = attune_core::error::VaultError::NotFound("nope".into());
        assert_eq!(classify_error_exit_code(&err), 1);
    }

    #[test]
    fn exit_code_model_load_maps_to_3() {
        let err = attune_core::error::VaultError::ModelLoad("no model".into());
        assert_eq!(classify_error_exit_code(&err), 3);
    }

    #[test]
    fn exit_code_io_not_found_maps_to_1() {
        let io_err = std::io::Error::new(std::io::ErrorKind::NotFound, "x");
        let err = attune_core::error::VaultError::Io(io_err);
        assert_eq!(classify_error_exit_code(&err), 1);
    }

    #[test]
    fn exit_code_other_io_maps_to_3() {
        let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "x");
        let err = attune_core::error::VaultError::Io(io_err);
        assert_eq!(classify_error_exit_code(&err), 3);
    }
}
