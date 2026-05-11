#!/usr/bin/env bash
# attune CLI 冒烟测试 — 30 秒验证核心 plugin / cloud 命令.
# 配套 scripts/smoke-test.sh (server 端).

set -e

ATTUNE="${ATTUNE_BIN:-$(pwd)/rust/target/release/attune}"
[ -x "$ATTUNE" ] || ATTUNE="$(pwd)/target/release/attune"
[ -x "$ATTUNE" ] || { echo "❌ attune binary not found"; exit 1; }

step() { echo -e "\n\033[36m━━━ $* ━━━\033[0m"; }
fail() { echo -e "\033[31m✗ $1\033[0m"; exit 1; }
pass() { echo -e "\033[32m✓ $1\033[0m"; }

step "1/7 keygen"
KEYDIR=$(mktemp -d)
$ATTUNE plugin-keygen --out-priv "$KEYDIR/key" > "$KEYDIR/out" 2>&1
[ -s "$KEYDIR/key" ] && pass "key written" || fail "key not written"
PUBKEY=$(grep "^PUBLIC_KEY=" "$KEYDIR/out" | cut -d= -f2)
[ "${#PUBKEY}" = "64" ] && pass "pubkey 64 hex" || fail "pubkey len ${#PUBKEY}"

step "2/7 sign + verify-sig"
PDIR=$(mktemp -d)
cat > "$PDIR/plugin.yaml" <<YAML
id: smoke-test
name: smoke
type: skill
version: 0.1.0
YAML
$ATTUNE plugin-sign "$PDIR" --priv-file "$KEYDIR/key" > /dev/null 2>&1
[ -s "$PDIR/plugin.sig" ] && pass "sig written" || fail "sig missing"
$ATTUNE plugin-verify-sig "$PDIR" "$PUBKEY" > /dev/null 2>&1 && pass "verify OK" || fail "verify failed"
$ATTUNE plugin-verify-sig "$PDIR" "0000000000000000000000000000000000000000000000000000000000000000" > /dev/null 2>&1 \
  && fail "wrong key should reject" || pass "wrong key rejected"

step "3/7 encrypt + decrypt"
ATTUNE_PLUGIN_KEY="smoke-test-key" $ATTUNE plugin-encrypt "$PDIR" > /dev/null 2>&1
[ -s "$PDIR/plugin.yaml.enc" ] && pass "encrypt OK" || fail "encrypt failed"
cp "$PDIR/plugin.yaml" "$PDIR/plugin.yaml.bak"
rm "$PDIR/plugin.yaml"
ATTUNE_PLUGIN_KEY="smoke-test-key" $ATTUNE plugin-decrypt "$PDIR" > /dev/null 2>&1
diff -q "$PDIR/plugin.yaml" "$PDIR/plugin.yaml.bak" > /dev/null && pass "decrypt 一致" || fail "decrypt 不一致"

step "4/7 plugin-list"
$ATTUNE plugin-list > /dev/null 2>&1 && pass "list OK" || fail "list failed"

step "5/7 link-folder"
TMPF=$(mktemp -d)
$ATTUNE link-folder "$TMPF" --project smoke > /dev/null 2>&1 && pass "link OK" || fail "link failed"

step "6/7 login bad URL"
if echo "" | timeout 5 $ATTUNE login fake@x.com --cloud-url http://127.0.0.1:1 > /dev/null 2>&1; then
  fail "login bad URL should fail"
else
  pass "login bad URL rejected"
fi

step "7/7 sync-plugins bad URL"
if timeout 5 $ATTUNE sync-plugins --cloud-url http://127.0.0.1:1 > /dev/null 2>&1; then
  fail "sync bad URL should fail"
else
  pass "sync bad URL rejected"
fi

rm -rf "$KEYDIR" "$PDIR" "$TMPF"
step "All CLI smoke tests PASSED ✓"
