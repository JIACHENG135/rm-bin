#!/usr/bin/env bash
#
# 发版：构建 → Developer ID 签名 → 校验
# （公证 + 装订暂时注释掉了，见下面「构建 + 签名」一节）
#
#   npm run release          完整发版
#   npm run release -- -c    只检查凭据，不构建
#
# 凭据全部从钥匙串和已安装的证书里推导，仓库里不留任何个人信息。
# 首次使用前存一次 App 专用密码（https://appleid.apple.com 生成）：
#
#   security add-generic-password -s "rm-bin-notary" -a "<你的 Apple ID>" -w
#
set -euo pipefail

SERVICE="rm-bin-notary"
CHECK_ONLY=false
[[ "${1:-}" == "-c" || "${1:-}" == "--check" ]] && CHECK_ONLY=true

cd "$(dirname "$0")/.."

die() { printf '\033[31m✗ %s\033[0m\n' "$1" >&2; exit 1; }
ok()  { printf '\033[32m✓\033[0m %s\n' "$1"; }
step(){ printf '\n\033[1m%s\033[0m\n' "$1"; }

# ————— 凭据 —————
# 签名身份：优先环境变量，否则取钥匙串里第一张 Developer ID Application
IDENTITY="${APPLE_SIGNING_IDENTITY:-$(
  security find-identity -v -p codesigning |
    awk -F'"' '/Developer ID Application/ { print $2; exit }'
)}"
[[ -n "$IDENTITY" ]] || die "钥匙串里没有 Developer ID Application 证书。
   在 https://developer.apple.com/account/resources/certificates/add 签发一张，
   或从备份导入：security import <备份>.p12 -k ~/Library/Keychains/login.keychain-db"

# 团队 ID 就写在身份名末尾的括号里
TEAM="${APPLE_TEAM_ID:-$(sed -n 's/.*(\([A-Z0-9]\{10\}\))$/\1/p' <<<"$IDENTITY")}"
[[ -n "$TEAM" ]] || die "无法从签名身份解析出 Team ID：$IDENTITY"

# 公证凭据存在钥匙串里，明文不进环境、不进日志
ACCOUNT="${APPLE_ID:-$(
  security find-generic-password -s "$SERVICE" 2>/dev/null |
    awk -F'"' '/"acct"<blob>=/ { print $4 }'
)}"
PASSWORD="${APPLE_PASSWORD:-$(security find-generic-password -s "$SERVICE" -w 2>/dev/null || true)}"
[[ -n "$ACCOUNT" && -n "$PASSWORD" ]] || die "钥匙串里没有公证凭据。先存一次：
   security add-generic-password -s \"$SERVICE\" -a \"<你的 Apple ID>\" -w"

ok "签名身份  $IDENTITY"
ok "Team ID   $TEAM"
ok "Apple ID  $ACCOUNT"
ok "公证密码  已从钥匙串读取"

if $CHECK_ONLY; then
  step "仅检查凭据，未构建。"
  exit 0
fi

# ————— 构建 + 签名（+ 公证 + 装订，已注释掉）—————
# Tauri 只在 APPLE_ID / APPLE_PASSWORD / APPLE_TEAM_ID 都存在时才会公证；
# 不传这三个，构建出的 .app 就只签名、不公证。公证耗时不定（几分钟到几小时），
# 需要恢复时把下面三行取消注释即可。
step "构建中（仅签名，未公证）…"
APPLE_SIGNING_IDENTITY="$IDENTITY" \
  npm run tauri build
# APPLE_ID="$ACCOUNT" \
# APPLE_PASSWORD="$PASSWORD" \
# APPLE_TEAM_ID="$TEAM" \

# ————— 校验 —————
APP="src-tauri/target/release/bundle/macos/RM Bin.app"
DMG="$(ls -t src-tauri/target/release/bundle/dmg/*.dmg 2>/dev/null | head -1 || true)"

step "校验产物"
failed=false

codesign --verify --deep --strict "$APP" 2>/dev/null \
  && ok "签名完整" || { printf '✗ 签名校验未通过\n'; failed=true; }

# 公证已注释掉（见上），所以不装订、不校验 Notarized Developer ID——
# 首次打开会被 Gatekeeper 拦一次，需要右键「打开」放行。

step "产物"
printf '  %s\n' "$APP"
[[ -n "$DMG" ]] && printf '  %s\n' "$DMG"

$failed && die "校验未全部通过，先别分发。" || printf '\n\033[32m发版就绪，可以分发。\033[0m\n'
