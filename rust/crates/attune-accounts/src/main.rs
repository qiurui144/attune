//! attune-accounts CLI — reference SaaS server.

use attune_accounts::{router, AccountsState};

#[tokio::main]
async fn main() {
    let host = std::env::var("ACCOUNTS_HOST").unwrap_or_else(|_| "127.0.0.1".into());
    let port: u16 = std::env::var("ACCOUNTS_PORT")
        .unwrap_or_else(|_| "18901".into())
        .parse()
        .expect("invalid ACCOUNTS_PORT");
    let addr = format!("{host}:{port}");

    let state = AccountsState::default();

    // 注入 license signing key (生产从 KMS, 这里 env hex)
    if let Ok(hex) = std::env::var("ATTUNE_LICENSE_SIGN_KEY") {
        match hex::decode(hex.trim()) {
            Ok(bytes) if bytes.len() == 32 => {
                let mut sk = [0u8; 32];
                sk.copy_from_slice(&bytes);
                state.set_signing_key(sk);
                eprintln!("✓ license signing key loaded from ATTUNE_LICENSE_SIGN_KEY env");
            }
            Ok(_) => eprintln!("⚠️  ATTUNE_LICENSE_SIGN_KEY must be 32 bytes (64 hex)"),
            Err(e) => eprintln!("⚠️  bad ATTUNE_LICENSE_SIGN_KEY hex: {e}"),
        }
    } else {
        eprintln!("⚠️  ATTUNE_LICENSE_SIGN_KEY not set — license generation disabled");
        eprintln!("    Generate offline: cargo run -p attune-cli -- plugin-keygen | grep PRIVATE");
    }

    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("attune-accounts (reference) listening on http://{addr}");
    eprintln!("  POST /api/v1/devices/register");
    eprintln!("  POST /api/v1/devices/{{id}}/deactivate");
    eprintln!("  GET  /api/v1/devices?account_id=...");
    eprintln!("  POST /api/v1/devices/verify");
    eprintln!("  POST /api/v1/admin/licenses/generate   (集体授权 / 手动激活码)");
    eprintln!("  POST /api/v1/licenses/activate          (客户端激活, 离线可校验)");
    eprintln!("  POST /api/v1/admin/llm/configure        (云端 OpenAI 接口配置)");
    eprintln!("  POST /api/v1/llm/endpoint               (用户拿云端分配 endpoint)");
    eprintln!();
    eprintln!("⚠️  Reference impl: in-memory state. Replace with PostgreSQL for production.");
    axum::serve(listener, app).await.expect("serve");
}
