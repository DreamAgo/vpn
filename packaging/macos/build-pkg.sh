#!/bin/sh
# 构建面向 vpn-cli 客户端的 macOS .pkg 安装包。
#
# 前置（需在 macOS 真机/CI 上执行，本仓库开发机仅做语法自检）：
#   - Xcode Command Line Tools（提供 pkgbuild / productbuild）。
#   - 已执行 `cargo build --release --workspace`，生成 target/release/vpn-cli。
#     注意：如需通用二进制（Intel + Apple Silicon），分别在两种 target 下构建后
#     用 `lipo -create` 合并，再放入 payload。
#
# 用法（从仓库根执行）：
#   sh packaging/macos/build-pkg.sh [VERSION]
#   例： sh packaging/macos/build-pkg.sh 0.1.0
#
# 产物： dist/vpn-cli-<version>.pkg
#
# 本脚本为脚手架，需在 macOS 上构建验证。
set -e

VERSION="${1:-0.1.0}"
IDENTIFIER="com.xeflow.vpn.cli.pkg"
INSTALL_LOCATION="/usr/local/bin"

# 以仓库根为基准（脚本位于 packaging/macos/）。
SCRIPT_DIR=$(cd "$(dirname "$0")" && pwd)
REPO_ROOT=$(cd "${SCRIPT_DIR}/../.." && pwd)

BIN_SRC="${REPO_ROOT}/target/release/vpn-cli"
BUILD_DIR="${REPO_ROOT}/dist/macos-build"
PAYLOAD_ROOT="${BUILD_DIR}/payload"
COMPONENT_PKG="${BUILD_DIR}/vpn-cli-component.pkg"
OUT_PKG="${REPO_ROOT}/dist/vpn-cli-${VERSION}.pkg"

if [ ! -f "${BIN_SRC}" ]; then
    echo "错误：未找到 ${BIN_SRC}，请先执行 cargo build --release --workspace" >&2
    exit 1
fi

# 1) 准备 payload：模拟安装后的目录结构。
rm -rf "${BUILD_DIR}"
mkdir -p "${PAYLOAD_ROOT}${INSTALL_LOCATION}"
cp "${BIN_SRC}" "${PAYLOAD_ROOT}${INSTALL_LOCATION}/vpn-cli"
chmod 0755 "${PAYLOAD_ROOT}${INSTALL_LOCATION}/vpn-cli"

# --- 代码签名（占位）：需真机 + Apple Developer ID 证书 -----------------
# codesign --force --options runtime \
#   --sign "Developer ID Application: YOUR NAME (TEAMID)" \
#   "${PAYLOAD_ROOT}${INSTALL_LOCATION}/vpn-cli"
# -------------------------------------------------------------------------

# 2) 构建组件包（含 postinstall 脚本）。
pkgbuild \
    --root "${PAYLOAD_ROOT}" \
    --identifier "${IDENTIFIER}" \
    --version "${VERSION}" \
    --scripts "${SCRIPT_DIR}/scripts" \
    --install-location "/" \
    "${COMPONENT_PKG}"

# 3) 用 productbuild 封装为可分发 .pkg。
productbuild \
    --package "${COMPONENT_PKG}" \
    "${OUT_PKG}"

# --- 安装包签名 + 公证（占位）：需真机 + 证书 + Apple ID --------------
# 安装包签名：
# productsign --sign "Developer ID Installer: YOUR NAME (TEAMID)" \
#   "${OUT_PKG}" "${OUT_PKG%.pkg}-signed.pkg"
#
# 公证（notarization）+ 装订（staple）：
# xcrun notarytool submit "${OUT_PKG}" \
#   --apple-id "you@example.com" --team-id "TEAMID" \
#   --password "APP_SPECIFIC_PASSWORD" --wait
# xcrun stapler staple "${OUT_PKG}"
# -------------------------------------------------------------------------

echo "已生成： ${OUT_PKG}"
echo "提示：未签名/未公证的 .pkg 在他机安装会被 Gatekeeper 拦截，分发前需签名 + 公证。"
