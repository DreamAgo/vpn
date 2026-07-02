#requires -Version 5.1
<#
.SYNOPSIS
    下载 WireGuard 官方 Wintun 发行包并把指定架构的 wintun.dll 放到
    desktop/src-tauri/resources/wintun.dll(Windows 构建前的依赖准备)。

.PARAMETER Arch
    目标架构:amd64(默认)/ arm64 / x86。

.PARAMETER Version
    Wintun 发行版本(默认 0.14.1)。

.EXAMPLE
    pwsh desktop/scripts/fetch-wintun.ps1
    pwsh desktop/scripts/fetch-wintun.ps1 -Arch arm64
#>
[CmdletBinding()]
param(
    [ValidateSet('amd64', 'arm64', 'x86')]
    [string]$Arch = 'amd64',
    [string]$Version = '0.14.1'
)

$ErrorActionPreference = 'Stop'

# resources 目录(相对本脚本:desktop/scripts/ -> desktop/src-tauri/resources/)
$resourcesDir = Join-Path $PSScriptRoot '..\src-tauri\resources'
$resourcesDir = [System.IO.Path]::GetFullPath($resourcesDir)
New-Item -ItemType Directory -Force -Path $resourcesDir | Out-Null

$dest = Join-Path $resourcesDir 'wintun.dll'
$url = "https://www.wintun.net/builds/wintun-$Version.zip"
$tmpZip = Join-Path ([System.IO.Path]::GetTempPath()) "wintun-$Version.zip"
$tmpDir = Join-Path ([System.IO.Path]::GetTempPath()) "wintun-$Version"

Write-Host "下载 $url ..."
Invoke-WebRequest -Uri $url -OutFile $tmpZip

if (Test-Path $tmpDir) { Remove-Item -Recurse -Force $tmpDir }
Expand-Archive -Path $tmpZip -DestinationPath $tmpDir -Force

$src = Join-Path $tmpDir "wintun\bin\$Arch\wintun.dll"
if (-not (Test-Path $src)) {
    throw "压缩包内未找到 $Arch 的 wintun.dll(期望路径:$src)"
}

Copy-Item -Path $src -Destination $dest -Force
Write-Host "已写入 $dest ($Arch, wintun $Version)"
Write-Host "提示:仅使用官方签名版本;请在分发前确认 Wintun 许可允许随应用再分发。"
