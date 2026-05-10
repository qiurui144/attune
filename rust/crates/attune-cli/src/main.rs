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
        pubkey: String,
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
        _ => {}
    }
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
        // Plugin 子命令在 run() 头部已 handle, 这里 unreachable
        Commands::PluginEncrypt { .. } | Commands::PluginDecrypt { .. } | Commands::PluginVerify { .. }
        | Commands::PluginKeygen { .. } | Commands::PluginSign { .. } | Commands::PluginVerifySig { .. } => {
            unreachable!("plugin commands handled before vault open")
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
