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
    let app = router(state);

    let listener = tokio::net::TcpListener::bind(&addr).await.expect("bind");
    eprintln!("attune-accounts (reference) listening on http://{addr}");
    eprintln!("  POST /api/v1/devices/register");
    eprintln!("  POST /api/v1/devices/{{id}}/deactivate");
    eprintln!("  GET  /api/v1/devices?account_id=...");
    eprintln!("  POST /api/v1/devices/verify");
    eprintln!();
    eprintln!("⚠️  Reference impl uses in-memory storage. Not for production.");
    axum::serve(listener, app).await.expect("serve");
}
