# I18N fixture corpus

Multilingual ingest + search fixtures for `i18n_ingest_search_test.rs`
(docs/TESTING.md §2.6, corpus code **I18N**, axis **B language i18n**).

These exercise the ingest path (`parse_bytes` → `from_utf8_lossy`) and the FTS
lexical layer (`JiebaTokenizer → LowerCaser → Stemmer(English)`) on scripts
beyond the Chinese+English the tokenizer was designed for. The point is to
**record real behavior**, not to assert that every script tokenizes well:
- CJK (Simplified/Traditional) → jieba segments.
- English → lowercased + stemmed.
- JP / KR / Arabic / Hebrew → jieba has no model for them; they fall through
  jieba's CJK-vs-non-CJK split, so the *lexical* layer may segment them poorly.
  Semantic recall is expected to come from the **vector layer** (bge-m3,
  multilingual). The test documents exactly which native-script queries hit the
  FTS layer and which do not (see the lexical support map printed by the test
  and committed in `reports/2026-06-08_test-expand-i18n.md`).

## Committed (valid UTF-8, diff cleanly)

| file | script | known markers |
|------|--------|---------------|
| `japanese.md` | Japanese (hiragana/katakana/kanji) | `JPMARKER`, `ALPHA_TOKEN_JP` |
| `korean.md` | Korean (hangul) | `KRMARKER`, `ALPHA_TOKEN_KR` |
| `traditional_chinese.md` | Traditional Chinese | `TWMARKER`, `ALPHA_TOKEN_TW` |
| `arabic_rtl.md` | Arabic (RTL) | `ARMARKER`, `ALPHA_TOKEN_AR` |
| `hebrew_rtl.txt` | Hebrew (RTL) | `HEMARKER`, `ALPHA_TOKEN_HE` |
| `emoji_heavy.md` | emoji + ZWJ/skin-tone + mixed | `EMOJIMARKER`, `RUSTACEAN_TOKEN`, `FINAL_EMOJI_TOKEN`, `中文检索` |

## Generated, NOT committed (binary / non-UTF8) — see `generate.sh`

| file | encoding | ASCII marker (must survive lossy decode) |
|------|----------|------------------------------------------|
| `gbk_simplified.txt` | GBK (legacy CN) | `ASCII_GBK_MARKER` |
| `shift_jis_japanese.txt` | Shift-JIS (legacy JP) | `ASCII_SJIS_MARKER` |

The test regenerates the two non-UTF8 files in-process (byte-for-byte identical
to `generate.sh`) so the suite is self-contained on a clean checkout.
