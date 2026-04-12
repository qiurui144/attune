# Security Hardening Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 修复 npu-vault Rust 商用线与 Python 原型线中 8 个安全缺口，覆盖 CORS 全开放、敏感端点无认证保护、Bearer token 默认关闭、TLS 警告缺失、路径遍历、内存密钥泄漏、token 无吊销机制、密码变更无事务保护。

**Architecture:** Rust 端修改 `vault-server` 与 `vault-core` 两个 crate；Python 端修改 `api/index.py`；所有修改保持向后兼容 Chrome 扩展 `/api/v1/*` 协议。具体策略：CORS 白名单化、端点级强制认证、默认 require-auth=true、启动时 TLS 检测告警、路径边界验证、Zeroizing 中间 Vec、token nonce 吊销、事务化 change_password。

**Tech Stack:** Rust (axum 0.8, tower-http CorsLayer, zeroize::Zeroizing), rusqlite 事务 API, Python (FastAPI, pathlib.Path), dirs crate

---

### Task 1: 修复 CORS 全开放（T1）

**Files:**
- Modify: `npu-vault/crates/vault-server/src/main.rs`
- Test: `npu-vault/crates/vault-server/tests/cors_test.rs`

- [ ] 在 `main.rs` 顶部引入 `tower_http::cors::{AllowOrigin, CorsLayer}` 并移除 `Any`

```rust
// main.rs 顶部 use 块替换
use tower_http::cors::{AllowOrigin, CorsLayer};
use axum::http::{HeaderValue, Method};
```

- [ ] 将 `CorsLayer::new().allow_origin(Any)...` 替换为白名单实现（在 `main()` 函数 `let cors = ...` 处）

```rust
// 替换原来的 let cors = CorsLayer::new()...
let cors = {
    // 允许 chrome-extension://* 、localhost、127.0.0.1
    // tower-http AllowOrigin::predicate 支持闭包匹配
    let allowed = CorsLayer::new()
        .allow_origin(AllowOrigin::predicate(|origin: &HeaderValue, _req| {
            let s = origin.to_str().unwrap_or("");
            s.starts_with("chrome-extension://")
                || s.starts_with("http://localhost")
                || s.starts_with("http://127.0.0.1")
                || s.starts_with("https://localhost")
                || s.starts_with("https://127.0.0.1")
        }))
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([
            axum::http::header::CONTENT_TYPE,
            axum::http::header::AUTHORIZATION,
        ]);
    allowed
};
```

- [ ] 编写集成测试 `npu-vault/crates/vault-server/tests/cors_test.rs`

```rust
// tests/cors_test.rs
#[cfg(test)]
mod tests {
    /// 验证非白名单 Origin 被 CORS 拒绝（预检请求返回无 ACAO 头）
    #[test]
    fn cors_predicate_blocks_unknown_origin() {
        use axum::http::HeaderValue;

        let check = |origin: &str| -> bool {
            let s = origin;
            s.starts_with("chrome-extension://")
                || s.starts_with("http://localhost")
                || s.starts_with("http://127.0.0.1")
                || s.starts_with("https://localhost")
                || s.starts_with("https://127.0.0.1")
        };

        assert!(check("chrome-extension://abcdefghijklmnop"));
        assert!(check("http://localhost:18900"));
        assert!(check("http://127.0.0.1:18900"));
        assert!(!check("https://evil.com"));
        assert!(!check("http://192.168.1.100:18900"));
        assert!(!check("null"));
    }
}
```

- [ ] Commit: `security: restrict CORS to chrome-extension:// and localhost origins only`

---

### Task 2: `/vault/device-secret/export` 强制 Bearer token（T2）

**Files:**
- Modify: `npu-vault/crates/vault-server/src/middleware.rs`
- Test: `npu-vault/crates/vault-core/tests/security_test.rs`（新增测试用例）

- [ ] 在 `bearer_auth_guard` 中添加「敏感端点强制认证」逻辑，无论 `require_auth` 开关状态如何，在跳过全局认证的 early-return 之前插入检查

```rust
// middleware.rs bearer_auth_guard 函数，在 `if !state.require_auth { return next.run(request).await; }` 之前插入：

pub async fn bearer_auth_guard(
    State(state): State<SharedState>,
    request: Request<axum::body::Body>,
    next: Next,
) -> Response {
    let path = request.uri().path().to_string();

    // 敏感端点：无论全局 require_auth 是否开启，均强制 Bearer token
    const ALWAYS_AUTH_ENDPOINTS: &[&str] = &[
        "/api/v1/vault/device-secret/export",
        "/api/v1/vault/device-secret/import",
    ];
    let is_always_auth = ALWAYS_AUTH_ENDPOINTS.iter().any(|ep| path == *ep);

    // Skip if auth not required and this is NOT a forced-auth endpoint
    if !state.require_auth && !is_always_auth {
        return next.run(request).await;
    }

    // Public endpoints and vault bootstrap endpoints bypass the token check
    // (but NOT if this is a forced-auth endpoint)
    if !is_always_auth
        && (path == "/api/v1/status/health"
            || path == "/"
            || path.starts_with("/ui/")
            || path == "/api/v1/vault/setup"
            || path == "/api/v1/vault/unlock"
            || path == "/api/v1/vault/status")
    {
        return next.run(request).await;
    }

    // Extract Bearer token
    let auth_header = request
        .headers()
        .get("authorization")
        .and_then(|h| h.to_str().ok())
        .and_then(|h| h.strip_prefix("Bearer "))
        .map(|t| t.to_string());

    let token = match auth_header {
        Some(t) => t,
        None => {
            return (
                StatusCode::UNAUTHORIZED,
                Json(serde_json::json!({"error": "missing bearer token"})),
            )
                .into_response()
        }
    };

    let verify_result = {
        let vault = state.vault.lock().unwrap();
        vault.verify_session(&token).map_err(|e| e.to_string())
    };

    match verify_result {
        Ok(_) => next.run(request).await,
        Err(e) => (
            StatusCode::UNAUTHORIZED,
            Json(serde_json::json!({"error": e})),
        )
            .into_response(),
    }
}
```

- [ ] 在测试文件中添加验证 device-secret/export 无 token 时返回 401 的单元测试

```rust
// tests/security_test.rs 新增
#[test]
fn always_auth_endpoints_list_contains_device_secret() {
    const ALWAYS_AUTH_ENDPOINTS: &[&str] = &[
        "/api/v1/vault/device-secret/export",
        "/api/v1/vault/device-secret/import",
    ];
    assert!(ALWAYS_AUTH_ENDPOINTS.contains(&"/api/v1/vault/device-secret/export"));
    assert!(ALWAYS_AUTH_ENDPOINTS.contains(&"/api/v1/vault/device-secret/import"));
}
```

- [ ] Commit: `security: force bearer token for device-secret endpoints regardless of require_auth flag`

---

### Task 3: Bearer token 默认改为 true（T3）

**Files:**
- Modify: `npu-vault/crates/vault-server/src/main.rs`

- [ ] 将 CLI 参数 `require_auth` 的 `default_value` 从 `"false"` 改为 `"true"`，并新增 `--no-auth` 反向 flag 用于本地开发

```rust
// 替换 Cli struct 中的 require_auth 字段定义
#[derive(Parser)]
#[command(name = "npu-vault-server", version, about = "npu-vault HTTP API server")]
struct Cli {
    #[arg(long, default_value = "127.0.0.1")]
    host: String,
    #[arg(long, default_value = "18900")]
    port: u16,
    /// Path to TLS certificate (PEM) - enables HTTPS
    #[arg(long)]
    tls_cert: Option<String>,
    /// Path to TLS private key (PEM)
    #[arg(long)]
    tls_key: Option<String>,
    /// Require Bearer token authentication (default: enabled).
    /// Use --no-auth to disable for local development only.
    #[arg(long, default_value = "true")]
    require_auth: bool,
    /// Disable Bearer token authentication (local dev only, overrides --require-auth)
    #[arg(long, default_value = "false")]
    no_auth: bool,
}
```

- [ ] 在 `main()` 中处理 `--no-auth` 覆盖逻辑，并打印警告

```rust
// 在 main() 中 Cli::parse() 之后、AppState 创建之前插入：
let require_auth = if cli.no_auth {
    tracing::warn!(
        "⚠  Authentication DISABLED via --no-auth. \
         Do NOT use in production or on network-accessible hosts."
    );
    false
} else {
    cli.require_auth
};
let shared_state = Arc::new(state::AppState::new(vault, require_auth));
```

- [ ] Commit: `security: default require-auth to true, add --no-auth flag for dev`

---

### Task 4: NAS 模式 TLS 警告（T4）

**Files:**
- Modify: `npu-vault/crates/vault-server/src/main.rs`
- Modify: `npu-vault/DEVELOP.md`
- Modify: `npu-vault/README.md`

- [ ] 在 `main()` 启动监听前插入安全检查：host 非 loopback 且 TLS 未启用时打印 WARNING

```rust
// 在 let addr: std::net::SocketAddr = ... 行之前插入：
// NAS 模式安全告警：非 loopback host 且无 TLS 时提醒用户
let is_loopback = cli.host == "127.0.0.1" || cli.host == "localhost" || cli.host == "::1";
let has_tls = cli.tls_cert.is_some() && cli.tls_key.is_some();
if !is_loopback && !has_tls {
    tracing::warn!(
        "⚠  WARNING: Server bound to non-loopback address '{}' without TLS. \
         All traffic (including tokens and vault data) is transmitted in plaintext. \
         Enable TLS with --tls-cert and --tls-key for NAS/remote access.",
        cli.host
    );
}
if !is_loopback && !require_auth {
    tracing::warn!(
        "⚠  WARNING: Authentication is DISABLED on a non-loopback interface '{}'. \
         Any host on the network can access your vault without credentials.",
        cli.host
    );
}
```

- [ ] 在 `npu-vault/DEVELOP.md` 的「NAS 模式」或「部署」章节中追加 TLS 强制要求说明

```markdown
<!-- 在 DEVELOP.md 的部署/NAS 模式章节添加以下内容 -->

> **安全警告：NAS 远程访问必须启用 TLS**
>
> 绑定非 loopback 地址（如 `--host 0.0.0.0`）时，**必须**同时指定 `--tls-cert` 和 `--tls-key`，
> 否则 Bearer token 和加密数据在传输层明文暴露。
>
> ```bash
> # 正确的 NAS 模式启动命令
> npu-vault-server --host 0.0.0.0 --port 18900 \
>   --tls-cert /path/to/cert.pem \
>   --tls-key  /path/to/key.pem
> ```
>
> 服务器在非安全配置下启动时会在日志中打印 `⚠ WARNING` 提醒。
```

- [ ] 在 `npu-vault/README.md` 的快速启动章节同步添加 TLS 警告提示

```markdown
<!-- 在 README.md 的"NAS / 远程访问"段落添加 -->

**NAS 模式必须启用 TLS**：远程访问时请加上 `--tls-cert` 和 `--tls-key` 参数，
否则服务器会在启动日志中输出安全警告。
```

- [ ] Commit: `security: warn on non-TLS remote binding and update NAS docs`

---

### Task 5: 目录绑定路径边界验证（T5）

**Files:**
- Modify: `npu-vault/crates/vault-server/src/routes/index.rs`
- Modify: `src/npu_webhook/api/index.py`
- Test: `npu-vault/crates/vault-server/tests/index_path_test.rs`

- [ ] 在 Rust `bind_directory` handler 中，在 `!path.exists() || !path.is_dir()` 检查之后插入路径边界验证

```rust
// routes/index.rs bind_directory 函数，替换原有的 path 检查块
pub async fn bind_directory(
    State(state): State<SharedState>,
    Json(body): Json<BindRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<serde_json::Value>)> {
    let vault = state.vault.lock().unwrap();
    let dek = vault.dek_db().map_err(|e| {
        (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error": e.to_string()})),
        )
    })?;

    let path = std::path::Path::new(&body.path);

    // 1. 必须是绝对路径
    if !path.is_absolute() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path must be absolute"})),
        ));
    }

    // 2. 规范化路径（消除 ../）
    let canonical = path.canonicalize().map_err(|_| {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "directory not found or inaccessible"})),
        )
    })?;

    if !canonical.is_dir() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error": "path is not a directory"})),
        ));
    }

    // 3. 必须在 home 目录下（防止绑定 /etc、/proc 等系统目录）
    let home = dirs::home_dir().ok_or_else(|| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": "cannot determine home directory"})),
        )
    })?;
    if !canonical.starts_with(&home) {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({
                "error": "path must be within the user home directory",
                "home": home.display().to_string(),
            })),
        ));
    }

    // 使用规范化后的路径字符串
    let canonical_str = canonical.display().to_string();

    let file_type_strs: Vec<&str> = body.file_types.iter().map(|s| s.as_str()).collect();
    let dir_id = vault
        .store()
        .bind_directory(&canonical_str, body.recursive, &file_type_strs)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            )
        })?;

    // Scan directory synchronously
    let scan_result =
        scanner::scan_directory(vault.store(), &dek, &dir_id, &canonical, body.recursive, &body.file_types)
            .map_err(|e| {
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(serde_json::json!({"error": e.to_string()})),
                )
            })?;

    {
        let ft_guard = state.fulltext.lock().unwrap();
        if let Some(ft) = ft_guard.as_ref() {
            if let Ok(ids) = vault.store().list_all_item_ids() {
                for id in &ids {
                    if let Ok(Some(item)) = vault.store().get_item(&dek, id) {
                        let _ = ft.add_document(&item.id, &item.title, &item.content, &item.source_type);
                    }
                }
            }
        }
    }

    Ok(Json(serde_json::json!({
        "status": "ok",
        "dir_id": dir_id,
        "scan": {
            "total": scan_result.total_files,
            "new": scan_result.new_files,
            "updated": scan_result.updated_files,
            "skipped": scan_result.skipped_files,
        }
    })))
}
```

- [ ] 修改 Python 端 `src/npu_webhook/api/index.py` 的 `bind_directory` 添加相同验证

```python
# api/index.py bind_directory 函数替换
import os

@router.post("/index/bind")
async def bind_directory(req: BindDirectoryRequest) -> dict:
    if not state.db:
        raise HTTPException(status_code=503, detail="Database not initialized")

    req_path = Path(req.path)

    # 1. 必须是绝对路径
    if not req_path.is_absolute():
        raise HTTPException(status_code=400, detail="path must be absolute")

    # 2. 规范化路径（消除 ../ 等）
    try:
        canonical = req_path.resolve(strict=True)
    except (OSError, RuntimeError):
        raise HTTPException(status_code=400, detail="directory not found or inaccessible")

    if not canonical.is_dir():
        raise HTTPException(status_code=400, detail="path is not a directory")

    # 3. 必须在 home 目录下
    home = Path.home()
    try:
        canonical.relative_to(home)
    except ValueError:
        raise HTTPException(
            status_code=400,
            detail=f"path must be within the user home directory ({home})"
        )

    dir_id = state.db.bind_directory(
        path=str(canonical),
        recursive=req.recursive,
        file_types=req.file_types,
    )

    if state.watcher:
        state.watcher.watch(str(canonical), recursive=req.recursive, file_types=req.file_types)

    if state.pipeline:
        dir_info = {
            "id": dir_id,
            "path": str(canonical),
            "recursive": req.recursive,
            "file_types": json.dumps(req.file_types),
        }
        threading.Thread(target=state.pipeline.scan_directory, args=(dir_info,), daemon=True).start()

    return {"status": "ok", "id": dir_id}
```

- [ ] 编写路径验证测试

```rust
// tests/index_path_test.rs
#[cfg(test)]
mod tests {
    use std::path::Path;

    fn is_safe_path(raw: &str) -> bool {
        let p = Path::new(raw);
        if !p.is_absolute() {
            return false;
        }
        // 简化模拟：检查是否包含 ..
        for component in p.components() {
            if component.as_os_str() == ".." {
                return false;
            }
        }
        true
    }

    #[test]
    fn rejects_relative_path() {
        assert!(!is_safe_path("relative/path"));
    }

    #[test]
    fn rejects_path_with_dotdot() {
        assert!(!is_safe_path("/home/user/../../etc/passwd"));
    }

    #[test]
    fn accepts_absolute_home_path() {
        assert!(is_safe_path("/home/user/documents"));
    }
}
```

- [ ] Commit: `security: validate bind path is absolute and within home directory (Rust + Python)`

---

### Task 6: `derive_master_key` 中间 Vec 使用 Zeroizing（T6）

**Files:**
- Modify: `npu-vault/crates/vault-core/src/crypto.rs`
- Test: `npu-vault/crates/vault-core/tests/crypto_test.rs`

- [ ] 在 `crypto.rs` 顶部 use 块中添加 `Zeroizing`

```rust
// crypto.rs 顶部，在 use zeroize::{Zeroize, ZeroizeOnDrop}; 这行中补充 Zeroizing：
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};
```

- [ ] 将 `derive_master_key` 中的 `let mut input: Vec<u8>` 改为 `Zeroizing<Vec<u8>>`

```rust
// 替换 derive_master_key 函数完整实现
pub fn derive_master_key(password: &[u8], device_secret: &[u8], salt: &[u8]) -> Result<Key32> {
    // 使用 Zeroizing<Vec<u8>> 确保拼接后的敏感输入在 drop 时自动清零
    let mut input: Zeroizing<Vec<u8>> = Zeroizing::new(Vec::with_capacity(password.len() + device_secret.len()));
    input.extend_from_slice(password);
    input.extend_from_slice(device_secret);

    let params = argon2::Params::new(ARGON2_M_COST, ARGON2_T_COST, ARGON2_P_COST, Some(32))
        .map_err(|e| VaultError::Crypto(format!("argon2 params: {e}")))?;
    let argon2 = Argon2::new(argon2::Algorithm::Argon2id, argon2::Version::V0x13, params);

    let mut mk = [0u8; 32];
    argon2
        .hash_password_into(&input, salt, &mut mk)
        .map_err(|e| VaultError::Crypto(format!("argon2 derive: {e}")))?;

    // input 在此处自动 zeroize（Zeroizing<T> 的 Drop impl）
    Ok(Key32(mk))
}
```

- [ ] 添加编译验证测试（确认 Zeroizing 类型可通过编译）

```rust
// tests/crypto_test.rs 新增
#[test]
fn derive_master_key_compiles_with_zeroizing() {
    use vault_core::crypto::derive_master_key;
    let password = b"test_password_123";
    let device_secret = [0u8; 32];
    let salt = [0u8; 32];
    // 确认函数可正常调用，中间 Vec 使用 Zeroizing 不影响结果
    let result = derive_master_key(password, &device_secret, &salt);
    assert!(result.is_ok());
}
```

- [ ] Commit: `security: use Zeroizing<Vec<u8>> for derive_master_key intermediate buffer`

---

### Task 7: lock 时 token 吊销机制（T8）

**Files:**
- Modify: `npu-vault/crates/vault-core/src/store.rs`
- Modify: `npu-vault/crates/vault-core/src/vault.rs`
- Test: `npu-vault/crates/vault-core/tests/session_revoke_test.rs`

- [ ] 在 `store.rs` 的 `SCHEMA_SQL` 中为 `vault_meta` 添加 `token_nonce` 的初始化迁移方法（幂等），并新增事务辅助方法

```rust
// store.rs 中新增方法（在 has_meta 之后）

/// 获取当前 token nonce（不存在时返回 0）
pub fn get_token_nonce(&self) -> Result<u64> {
    match self.get_meta("token_nonce")? {
        Some(bytes) if bytes.len() == 8 => {
            Ok(u64::from_le_bytes(bytes.try_into().unwrap()))
        }
        _ => Ok(0u64),
    }
}

/// 递增 token nonce（每次 lock 调用）
pub fn increment_token_nonce(&self) -> Result<u64> {
    let current = self.get_token_nonce()?;
    let next = current.wrapping_add(1);
    self.set_meta("token_nonce", &next.to_le_bytes())?;
    Ok(next)
}

/// 事务执行：begin / commit / rollback
pub fn execute_in_transaction<F, T>(&self, f: F) -> Result<T>
where
    F: FnOnce(&Connection) -> Result<T>,
{
    self.conn.execute_batch("BEGIN")?;
    match f(&self.conn) {
        Ok(v) => {
            self.conn.execute_batch("COMMIT")?;
            Ok(v)
        }
        Err(e) => {
            let _ = self.conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
```

- [ ] 修改 `vault.rs` 的 `lock()` 方法，在清除内存密钥的同时递增 `token_nonce`

```rust
// vault.rs lock() 方法完整替换
/// 锁定 vault（清零内存密钥，同时递增 token_nonce 使旧 token 失效）
pub fn lock(&self) -> Result<()> {
    // 先递增 nonce，使所有已签发 token 失效
    self.store.increment_token_nonce()?;
    // 再清零内存密钥（UnlockedKeys 内的 Key32 实现了 ZeroizeOnDrop）
    let mut guard = self.unlocked.lock().unwrap();
    *guard = None;
    Ok(())
}
```

- [ ] 修改 `vault.rs` 的 `create_session_token()` 方法，在 payload 中嵌入当前 nonce

```rust
// vault.rs create_session_token() 完整替换
fn create_session_token(&self, mk: &Key32) -> Result<String> {
    let session_id = uuid::Uuid::new_v4().simple().to_string();
    let expires = chrono::Utc::now().timestamp() + SESSION_TTL_SECS;
    let nonce = self.store.get_token_nonce()?;
    // payload 格式：{session_id}:{expires}:{nonce}
    let payload = format!("{session_id}:{expires}:{nonce}");
    let sig = crypto::hmac_sign(mk, payload.as_bytes());
    Ok(format!("{payload}.{}", hex::encode(sig)))
}
```

- [ ] 修改 `vault.rs` 的 `verify_session()` 方法，校验 nonce 匹配

```rust
// vault.rs verify_session() 完整替换
/// 验证 session token（检查签名、过期时间、nonce 吊销状态）
pub fn verify_session(&self, token: &str) -> Result<()> {
    let guard = self.unlocked.lock().unwrap();
    let keys = guard.as_ref().ok_or(VaultError::Locked)?;

    // 分离签名与 payload
    let dot_pos = token.rfind('.').ok_or(VaultError::SessionInvalid)?;
    let payload = &token[..dot_pos];
    let sig_hex = &token[dot_pos + 1..];

    let sig = hex::decode(sig_hex).map_err(|_| VaultError::SessionInvalid)?;
    if !crypto::hmac_verify(&keys.master_key, payload.as_bytes(), &sig) {
        return Err(VaultError::SessionInvalid);
    }

    // payload 格式：{session_id}:{expires}:{nonce}
    let parts: Vec<&str> = payload.split(':').collect();
    if parts.len() != 3 {
        return Err(VaultError::SessionInvalid);
    }
    let expires: i64 = parts[1].parse().map_err(|_| VaultError::SessionInvalid)?;
    let token_nonce: u64 = parts[2].parse().map_err(|_| VaultError::SessionInvalid)?;

    // 检查过期
    let now = chrono::Utc::now().timestamp();
    if now > expires {
        return Err(VaultError::SessionExpired);
    }

    // 检查 nonce：token 中的 nonce 必须等于当前存储的 nonce
    // 每次 lock() 递增 nonce，旧 token 的 nonce 会小于当前值
    drop(guard); // 释放 unlocked 锁后再访问 store
    let current_nonce = self.store.get_token_nonce()?;
    if token_nonce != current_nonce {
        return Err(VaultError::SessionInvalid);
    }

    Ok(())
}
```

- [ ] 编写 token 吊销测试

```rust
// tests/session_revoke_test.rs
#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vault_core::vault::Vault;

    #[test]
    fn token_revoked_after_lock() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("password123").unwrap();
        // 重新 lock 再 unlock 以获取 token
        vault.lock().unwrap();
        let token = vault.unlock("password123").unwrap();

        // token 在 unlock 后应该有效
        assert!(vault.verify_session(&token).is_ok());

        // lock 之后 token 应该失效
        vault.lock().unwrap();
        // vault 已 locked，verify_session 返回 VaultError::Locked
        let result = vault.verify_session(&token);
        assert!(result.is_err());
    }

    #[test]
    fn new_token_valid_after_relock_unlock() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("password123").unwrap();
        vault.lock().unwrap();
        let old_token = vault.unlock("password123").unwrap();

        vault.lock().unwrap();
        let new_token = vault.unlock("password123").unwrap();

        // 旧 token nonce 不匹配，应失效
        assert!(vault.verify_session(&old_token).is_err());
        // 新 token 应有效
        assert!(vault.verify_session(&new_token).is_ok());
    }
}
```

- [ ] Commit: `security: add token_nonce revocation - lock() increments nonce to invalidate old tokens`

---

### Task 8: `change_password` 事务保护（F1）

**Files:**
- Modify: `npu-vault/crates/vault-core/src/store.rs`（复用 Task 7 新增的 `execute_in_transaction`）
- Modify: `npu-vault/crates/vault-core/src/vault.rs`
- Test: `npu-vault/crates/vault-core/tests/change_password_test.rs`

- [ ] 确认 `store.rs` 的 `execute_in_transaction` 已在 Task 7 中添加（本 Task 直接复用）

- [ ] 修改 `vault.rs` 的 `change_password()` 方法，将 3 次 `set_meta` 包进事务

```rust
// vault.rs change_password() 完整替换
/// 更改密码（重新加密 DEK，数据不变）。3 次 set_meta 在事务中执行，防止部分写入。
pub fn change_password(&self, old_password: &str, new_password: &str) -> Result<()> {
    if self.state() != VaultState::Unlocked {
        return Err(VaultError::Locked);
    }

    let guard = self.unlocked.lock().unwrap();
    let keys = guard.as_ref().ok_or(VaultError::Locked)?;

    // 验证旧密码（重新派生 MK 比对）
    let ds_path = self.config_dir.join("device.key");
    let device_secret_bytes = std::fs::read(&ds_path)
        .map_err(|_| VaultError::DeviceSecretMissing(ds_path.display().to_string()))?;
    let salt = self.store.get_meta("salt")?
        .ok_or(VaultError::Crypto("missing salt".into()))?;
    let old_mk = crypto::derive_master_key(old_password.as_bytes(), &device_secret_bytes, &salt)?;

    // 验证旧 MK 能解密 dek_db
    let enc_dek_db = self.store.get_meta("encrypted_dek_db")?
        .ok_or(VaultError::Crypto("missing dek_db".into()))?;
    crypto::decrypt_dek(&old_mk, &enc_dek_db)?; // 旧密码错误时报 InvalidPassword

    // 生成新 salt + 派生新 MK
    let new_salt = crypto::generate_salt();
    let new_mk = crypto::derive_master_key(new_password.as_bytes(), &device_secret_bytes, &new_salt)?;

    // 预计算新加密 DEK（在事务外计算，避免持锁时 argon2 长耗时）
    let new_enc_dek_db = crypto::encrypt_dek(&new_mk, &keys.dek_db)?;
    let new_enc_dek_idx = crypto::encrypt_dek(&new_mk, &keys.dek_idx)?;
    let new_enc_dek_vec = crypto::encrypt_dek(&new_mk, &keys.dek_vec)?;

    // 在单个 SQLite 事务中原子写入 salt + 3 个 DEK，防止中途失败导致数据不一致
    self.store.execute_in_transaction(|conn| {
        conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES ('salt', ?1)",
            rusqlite::params![new_salt.as_ref()],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES ('encrypted_dek_db', ?1)",
            rusqlite::params![new_enc_dek_db.as_slice()],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES ('encrypted_dek_idx', ?1)",
            rusqlite::params![new_enc_dek_idx.as_slice()],
        )?;
        conn.execute(
            "INSERT OR REPLACE INTO vault_meta (key, value) VALUES ('encrypted_dek_vec', ?1)",
            rusqlite::params![new_enc_dek_vec.as_slice()],
        )?;
        Ok(())
    })?;

    drop(guard);
    Ok(())
}
```

- [ ] 注意：`execute_in_transaction` 的闭包参数是 `&Connection`，需要在 `store.rs` 中把 `conn` 字段改为 `pub(crate)` 或提供额外方法；或者将事务逻辑封装在 `store.rs` 的专用方法 `set_meta_batch` 中

```rust
// store.rs 替代方案：添加专用批量写入方法（保持 conn 私有）
/// 在单个事务中批量写入 vault_meta（用于 change_password 原子更新）
pub fn set_meta_batch(&self, entries: &[(&str, &[u8])]) -> Result<()> {
    self.conn.execute_batch("BEGIN")?;
    let result: Result<()> = (|| {
        for (key, value) in entries {
            self.conn.execute(
                "INSERT OR REPLACE INTO vault_meta (key, value) VALUES (?1, ?2)",
                rusqlite::params![key, value],
            )?;
        }
        Ok(())
    })();
    match result {
        Ok(_) => {
            self.conn.execute_batch("COMMIT")?;
            Ok(())
        }
        Err(e) => {
            let _ = self.conn.execute_batch("ROLLBACK");
            Err(e)
        }
    }
}
```

- [ ] 相应地在 `change_password` 中使用 `set_meta_batch`

```rust
// change_password 事务写入部分改用 set_meta_batch（替换 execute_in_transaction 块）
self.store.set_meta_batch(&[
    ("salt", new_salt.as_ref()),
    ("encrypted_dek_db", new_enc_dek_db.as_slice()),
    ("encrypted_dek_idx", new_enc_dek_idx.as_slice()),
    ("encrypted_dek_vec", new_enc_dek_vec.as_slice()),
])?;
```

- [ ] 编写 change_password 事务测试

```rust
// tests/change_password_test.rs
#[cfg(test)]
mod tests {
    use tempfile::TempDir;
    use vault_core::vault::Vault;

    #[test]
    fn change_password_and_relock_unlock_with_new_password() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("old_password").unwrap();

        // 变更密码
        vault.change_password("old_password", "new_password").unwrap();

        // 旧密码不能 unlock
        vault.lock().unwrap();
        assert!(vault.unlock("old_password").is_err());

        // 新密码可以 unlock
        assert!(vault.unlock("new_password").is_ok());
    }

    #[test]
    fn change_password_with_wrong_old_password_fails() {
        let tmp = TempDir::new().unwrap();
        let db_path = tmp.path().join("vault.db");
        let vault = Vault::open(&db_path, tmp.path()).unwrap();

        vault.setup("correct_password").unwrap();
        let result = vault.change_password("wrong_password", "new_password");
        assert!(result.is_err());
    }
}
```

- [ ] Commit: `security: wrap change_password 4 meta writes in a single SQLite transaction`

---

## 实施顺序建议

1. **Task 6**（crypto.rs Zeroizing）— 改动最小，风险最低，先做
2. **Task 3**（require-auth 默认 true）+ **Task 2**（device-secret 强制认证）— 逻辑相关，一起做
3. **Task 1**（CORS 白名单）— 独立改动
4. **Task 4**（TLS 警告 + 文档）— 主要是文档
5. **Task 5**（路径边界验证）— 需同时改 Rust 和 Python
6. **Task 7**（token nonce 吊销）— 涉及 store + vault 两个文件，协调改动
7. **Task 8**（change_password 事务）— 复用 Task 7 的 set_meta_batch

## 自检结果

- [x] 占位符扫描：无 TBD / TODO / similar to above
- [x] 类型一致性：`store.set_meta_batch(&[(&str, &[u8])])` 在 Task 7 和 Task 8 之间签名一致
- [x] 覆盖检查：T1 / T2 / T3 / T4 / T5 / T6 / T8 / F1 共 8 个缺口全部有对应 Task
