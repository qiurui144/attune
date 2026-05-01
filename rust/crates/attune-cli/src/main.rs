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
    Ocr {
        /// Image path (PNG / JPG / etc supported by `image` crate)
        image: std::path::PathBuf,
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
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> attune_core::error::Result<()> {
    // OCR / Deploy 子命令不需要 vault — 早 return 避免 zero-state 报错
    if let Commands::Ocr { image } = &cli.command {
        let provider = attune_core::ocr::detect_default_provider().ok_or_else(|| {
            attune_core::error::VaultError::ModelLoad(
                "PP-OCR models missing — run `attune deploy` or apt install --reinstall attune".into(),
            )
        })?;
        eprintln!("[attune ocr] engine: {} | image: {}", provider.name(), image.display());
        let start = std::time::Instant::now();
        let text = provider.extract_text_from_image(image)?;
        eprintln!("[attune ocr] {:?} elapsed", start.elapsed());
        println!("{text}");
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
            vault.setup(&password)?;
            println!("Vault initialized and unlocked.");
            println!("Device secret saved to: {}", attune_core::platform::device_secret_path().display());
            println!("IMPORTANT: Back up your device.key file — you need it to unlock on other devices.");
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
        Commands::VaultImport { src, force } => {
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
        }
    }
    Ok(())
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
