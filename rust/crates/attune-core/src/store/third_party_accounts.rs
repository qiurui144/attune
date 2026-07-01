//! 通用第三方账号凭据保险柜（弱腿增量波 C）。
//!
//! 与 store/email_accounts.rs / store/git_sources.rs 同加密模式：`secret`
//! （密码 / token / app-password）经字段级 AES-256-GCM（dek）加密落 `secret_enc`
//! BLOB 列，**明文凭据绝不落盘**。`username` / `endpoint` / `label` 不算 secret，
//! 明文存储以便 UI 列表脱敏显示（只回显「连接 id + provider + 用户名 + 状态」，
//! 永不回显 secret）。
//!
//! 区别于 email_accounts：那是「已挂成采集源」的账户（绑 bound_dirs.id）；本表是
//! 「连接前」的通用凭据登记层 —— 用户先存一份源凭据，suggestions 规则引擎据此知道
//! 「用户已连接哪些源」从而不再弹「连接账号」建议卡。
//!
//! 所有方法属于 `impl Store`（inherent impl 跨文件分裂，rustc 自动合并）。

use rusqlite::params;

use crate::crypto::{self, Key32};
use crate::error::{Result, VaultError};
use crate::store::Store;

/// 已知第三方源 provider 允许集。新 provider 必须先入此白名单 —— 拒绝 typo 静默写入
/// unknown provider 让 suggestions 规则引擎「按 provider 判稀缺」永远算错。
pub const KNOWN_PROVIDERS: &[&str] = &[
    "webdav",        // 网盘 WebDAV
    "imap",          // 邮箱 IMAP
    "rss",           // RSS / Atom（带鉴权的私有 feed）
    "git",           // git 仓 token
    "browser_login", // INT-1: 浏览器登入协助会话（storage_state JSON 经 DEK 加密落 secret_enc）
    "other",         // 其他 OpenAI-compat / 自定义源
];

pub fn is_known_provider(provider: &str) -> bool {
    KNOWN_PROVIDERS.contains(&provider)
}

/// 通用凭据 secret 上限（password / token / app-password 足够）。
const MAX_SECRET_LEN: usize = 8192;
/// `browser_login` 会话上限：Playwright `storage_state` JSON（cookies + localStorage）
/// 可达数十 KB。放宽到 256 KB；超限**报错而非截断**（截断会破坏 JSON → 会话失效）。
const MAX_BROWSER_LOGIN_SECRET_LEN: usize = 256 * 1024;

/// provider 对应的 secret 字节上限。`browser_login` 走会话上限，其余走通用上限。
fn max_secret_len(provider: &str) -> usize {
    if provider == "browser_login" {
        MAX_BROWSER_LOGIN_SECRET_LEN
    } else {
        MAX_SECRET_LEN
    }
}

/// 写入用的第三方账号配置（明文 secret，调用方持有；落库前由本模块加密）。
#[derive(Debug, Clone)]
pub struct ThirdPartyAccountInput {
    /// provider kind，必须 ∈ [`KNOWN_PROVIDERS`]。
    pub provider: String,
    /// 用户可读标签（"我的 Nextcloud"）；不算 secret，明文存。
    pub label: String,
    /// 用户名 / 账号（不算 secret，明文存，便于 UI 列表显示）。
    pub username: String,
    /// 服务端点（host / URL；不算 secret）。
    pub endpoint: String,
    /// 明文密码 / token / app-password —— 落库前用 dek 加密成 secret_enc BLOB。
    pub secret: String,
}

/// 脱敏视图：**绝不含 secret 明文**。API 只回这个。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThirdPartyAccountView {
    pub id: String,
    pub provider: String,
    pub label: String,
    pub username: String,
    pub endpoint: String,
    pub status: String,
    pub created_at: String,
    pub updated_at: String,
}

/// 校验输入：provider 必须已知、secret 不能空、endpoint 不能空。
fn validate(input: &ThirdPartyAccountInput) -> Result<()> {
    if !is_known_provider(&input.provider) {
        return Err(VaultError::Crypto(format!(
            "unknown third-party provider: {:?} (typo guard)",
            input.provider
        )));
    }
    if input.secret.is_empty() {
        return Err(VaultError::Crypto("secret must not be empty".into()));
    }
    if input.endpoint.trim().is_empty() {
        return Err(VaultError::Crypto("endpoint must not be empty".into()));
    }
    // defense-in-depth：超长字段会膨胀表 / 拖慢解密。上限按 provider 区分
    // （browser_login 会话 JSON 远大于普通凭据）。超限报错**不截断**。
    let limit = max_secret_len(&input.provider);
    if input.secret.len() > limit {
        return Err(VaultError::Crypto(format!(
            "secret too long (max {limit} for provider {:?})",
            input.provider
        )));
    }
    Ok(())
}

impl Store {
    /// 新增一条第三方账号凭据。`secret` 用 dek 加密成 BLOB 落盘，返回生成的 id。
    pub fn add_third_party_account(
        &self,
        dek: &Key32,
        input: &ThirdPartyAccountInput,
    ) -> Result<String> {
        validate(input)?;
        let id = format!("tpa-{}", uuid::Uuid::new_v4());
        let secret_enc = crypto::encrypt(dek, input.secret.as_bytes())?;
        let now = chrono::Utc::now().to_rfc3339();
        self.conn.execute(
            "INSERT INTO third_party_accounts
                (id, provider, label, username, endpoint, secret_enc, status, created_at, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, 'connected', ?7, ?7)",
            params![
                id,
                input.provider,
                input.label,
                input.username,
                input.endpoint,
                secret_enc,
                now,
            ],
        )?;
        Ok(id)
    }

    /// 列出全部第三方账号（**脱敏，不含 secret**）。UI / suggestions 用。
    pub fn list_third_party_accounts(&self) -> Result<Vec<ThirdPartyAccountView>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, provider, label, username, endpoint, status, created_at, updated_at
             FROM third_party_accounts ORDER BY created_at",
        )?;
        let rows = stmt.query_map([], |r| {
            Ok(ThirdPartyAccountView {
                id: r.get(0)?,
                provider: r.get(1)?,
                label: r.get(2)?,
                username: r.get(3)?,
                endpoint: r.get(4)?,
                status: r.get(5)?,
                created_at: r.get(6)?,
                updated_at: r.get(7)?,
            })
        })?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// distinct provider 集合（suggestions 规则引擎判「哪类源已连接」用）。
    pub fn connected_provider_kinds(&self) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT DISTINCT provider FROM third_party_accounts ORDER BY provider",
        )?;
        let rows = stmt.query_map([], |r| r.get::<_, String>(0))?;
        let mut out = Vec::new();
        for row in rows {
            out.push(row?);
        }
        Ok(out)
    }

    /// 读单条账户的解密后 secret（仅采集 worker / 同步用，**绝不进 API 响应**）。
    pub fn get_third_party_secret(&self, dek: &Key32, id: &str) -> Result<Option<String>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT secret_enc FROM third_party_accounts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        match blob {
            None => Ok(None),
            Some(secret_enc) => {
                let secret = String::from_utf8(crypto::decrypt(dek, &secret_enc)?)
                    .map_err(|e| VaultError::Crypto(format!("secret utf8: {e}")))?;
                Ok(Some(secret))
            }
        }
    }

    /// 删除一条第三方账号凭据。返回是否真的删了一行（不存在返回 false，非错误）。
    pub fn delete_third_party_account(&self, id: &str) -> Result<bool> {
        let n = self.conn.execute(
            "DELETE FROM third_party_accounts WHERE id = ?1",
            params![id],
        )?;
        Ok(n > 0)
    }

    /// 更新连接状态（"connected" / "error" / "expired"）。账户不存在返回 false。
    pub fn set_third_party_status(&self, id: &str, status: &str) -> Result<bool> {
        let now = chrono::Utc::now().to_rfc3339();
        let n = self.conn.execute(
            "UPDATE third_party_accounts SET status = ?1, updated_at = ?2 WHERE id = ?3",
            params![status, now, id],
        )?;
        Ok(n > 0)
    }

    /// third_party_accounts 表迁移（幂等）。纯追加表：老 vault 下次 open 自动建空表，
    /// SCHEMA_VERSION 不 bump（与 migrate_suggestions 同级）。`secret_enc` 是字段级
    /// AES-256-GCM 密文 BLOB —— 明文凭据绝不落盘。
    pub(crate) fn migrate_third_party_accounts(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS third_party_accounts (\
               id TEXT PRIMARY KEY, \
               provider TEXT NOT NULL, \
               label TEXT NOT NULL DEFAULT '', \
               username TEXT NOT NULL DEFAULT '', \
               endpoint TEXT NOT NULL DEFAULT '', \
               secret_enc BLOB NOT NULL, \
               status TEXT NOT NULL DEFAULT 'connected', \
               created_at TEXT NOT NULL, \
               updated_at TEXT NOT NULL);\
             CREATE INDEX IF NOT EXISTS idx_tpa_provider ON third_party_accounts(provider);",
        )?;
        Ok(())
    }

    /// 仅供集成测试用：取 secret_enc 原始密文字节（验证不含明文）。
    #[cfg(any(test, feature = "test-utils"))]
    #[doc(hidden)]
    pub fn debug_raw_third_party_secret_enc(&self, id: &str) -> Result<Vec<u8>> {
        let blob: Option<Vec<u8>> = self
            .conn
            .query_row(
                "SELECT secret_enc FROM third_party_accounts WHERE id = ?1",
                params![id],
                |r| r.get(0),
            )
            .ok();
        Ok(blob.unwrap_or_default())
    }
}
