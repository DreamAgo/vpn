# build-installer.ps1：在 Windows 上调用 Inno Setup 编译器 (ISCC.exe) 生成安装器。
#
# 前置（需 Windows 真机/CI，本仓库开发机为 macOS，无法运行）：
#   - 安装 Inno Setup 6+：https://jrsoftware.org/isdl.php
#   - 已执行 `cargo build --release --workspace`，生成 target\release\vpn-cli.exe
#
# 用法（PowerShell，从仓库根或任意目录）：
#   pwsh packaging\windows\build-installer.ps1 -Version 0.1.0
#
# 产物： dist\vpn-cli-setup-<version>.exe
#
# 本脚本为脚手架，需在 Windows 上运行验证。

param(
    [string]$Version = "0.1.0",
    # ISCC.exe 路径；默认尝试常见安装位置与 PATH。
    [string]$Iscc = ""
)

$ErrorActionPreference = "Stop"

# 脚本所在目录（packaging\windows\）与仓库根。
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RepoRoot  = Resolve-Path (Join-Path $ScriptDir "..\..")
$IssFile   = Join-Path $ScriptDir "vpn-cli.iss"
$Binary    = Join-Path $RepoRoot "target\release\vpn-cli.exe"

if (-not (Test-Path $Binary)) {
    throw "未找到 $Binary，请先执行： cargo build --release --workspace"
}

# 定位 ISCC.exe。
if ([string]::IsNullOrEmpty($Iscc)) {
    $candidates = @(
        "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
        "C:\Program Files\Inno Setup 6\ISCC.exe"
    )
    foreach ($c in $candidates) {
        if (Test-Path $c) { $Iscc = $c; break }
    }
    if ([string]::IsNullOrEmpty($Iscc)) {
        $cmd = Get-Command ISCC.exe -ErrorAction SilentlyContinue
        if ($cmd) { $Iscc = $cmd.Source }
    }
}

if ([string]::IsNullOrEmpty($Iscc) -or -not (Test-Path $Iscc)) {
    throw "未找到 ISCC.exe，请安装 Inno Setup 或用 -Iscc 指定路径。"
}

# 确保输出目录存在。
$DistDir = Join-Path $RepoRoot "dist"
if (-not (Test-Path $DistDir)) {
    New-Item -ItemType Directory -Path $DistDir | Out-Null
}

# 编译（通过 /D 覆盖 .iss 中的版本宏）。
& $Iscc "/DMyAppVersion=$Version" $IssFile
if ($LASTEXITCODE -ne 0) {
    throw "ISCC 编译失败，退出码 $LASTEXITCODE"
}

Write-Host "已生成安装器： $DistDir\vpn-cli-setup-$Version.exe"

# --- 代码签名（占位）：需真机 + 代码签名证书 (signtool) -------------------
# & signtool sign /fd SHA256 /tr http://timestamp.digicert.com /td SHA256 `
#   /a "$DistDir\vpn-cli-setup-$Version.exe"
# --------------------------------------------------------------------------
