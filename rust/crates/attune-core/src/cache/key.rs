//! BLAKE3 cache key derivation.
//!
//! 32-hex prefix of `blake3(model || 0xFF || prompt)`. 32 hex chars = 128 bits
//! of collision domain — comfortably above the 64-bit "treat as miss"
//! threshold called out in spec §7.2 (cache key collision behavior).
//!
//! The `0xFF` separator is a single byte that cannot appear in valid UTF-8
//! continuation bytes anywhere except as the start of a (never-valid) 5+ byte
//! sequence, so `("ab", "c")` and `("a", "bc")` produce distinct hashes
//! without needing a length prefix.

use blake3::Hasher;

/// Compute the 32-hex (128-bit) cache key for a `(model, prompt)` pair.
///
/// The result is always 32 lowercase ASCII hex characters.
pub fn cache_key(model: &str, prompt: &str) -> String {
    let mut h = Hasher::new();
    h.update(model.as_bytes());
    h.update(&[0xFF]);
    h.update(prompt.as_bytes());
    let full = h.finalize().to_hex().to_string();
    full[..32].to_string()
}
