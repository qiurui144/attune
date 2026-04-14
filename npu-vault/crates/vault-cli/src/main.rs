use clap::{Parser, Subcommand};
use vault_core::vault::Vault;

#[derive(Parser)]
#[command(name = "npu-vault", version, about = "Encrypted personal knowledge vault")]
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
}

fn main() {
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("error: {e}");
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> vault_core::error::Result<()> {
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
            println!("Device secret saved to: {}", vault_core::platform::device_secret_path().display());
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
            let count = if matches!(state, vault_core::vault::VaultState::Unlocked) {
                vault.store().item_count().unwrap_or(0)
            } else {
                0
            };
            let status = serde_json::json!({
                "state": state,
                "items": count,
                "data_dir": vault_core::platform::data_dir(),
                "config_dir": vault_core::platform::config_dir(),
            });
            println!("{}", serde_json::to_string_pretty(&status).unwrap());
        }
        Commands::Insert { title, content, source_type } => {
            let dek = vault.dek_db()?;
            let id = vault.store().insert_item(&dek, &title, &content, None, &source_type, None, None)?;
            println!("Inserted: {id}");
        }
        Commands::Get { id } => {
            let dek = vault.dek_db()?;
            match vault.store().get_item(&dek, &id)? {
                Some(item) => println!("{}", serde_json::to_string_pretty(&item).unwrap()),
                None => {
                    eprintln!("Item not found: {id}");
                    std::process::exit(1);
                }
            }
        }
        Commands::List { limit } => {
            let _ = vault.dek_db()?;
            let items = vault.store().list_items(limit, 0)?;
            println!("{}", serde_json::to_string_pretty(&items).unwrap());
        }
    }
    Ok(())
}

fn read_password(prompt: &str) -> vault_core::error::Result<String> {
    eprint!("{prompt}");
    rpassword::read_password().map_err(vault_core::error::VaultError::Io)
}
