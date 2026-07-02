; Inno Setup 脚本：面向客户端的 vpn-cli Windows 安装器。
;
; 功能：
;   - 安装 vpn-cli.exe 到 Program Files\VPN CLI。
;   - 将安装目录加入系统 PATH（通过 ChangesEnvironment + Path 修改）。
;   - 可选任务：注册 Windows Service（调用 `vpn-cli daemon install`）。
;
; 前置：在 Windows 上安装 Inno Setup（提供 ISCC.exe）。
;   编译： ISCC.exe packaging\windows\vpn-cli.iss
;   （或用同目录 build-installer.ps1）。
;
; 二进制前提：已执行 `cargo build --release --workspace`，
;   生成 target\release\vpn-cli.exe。
;
; 本脚本为脚手架，需在 Windows 上编译验证。

#define MyAppName "VPN CLI"
#define MyAppVersion "0.1.0"
#define MyAppPublisher "Shangguanjunjie"
#define MyAppURL "https://github.com/Shangguanjunjie/vpn"
#define MyAppExeName "vpn-cli.exe"

[Setup]
AppId={{9D2B5A1E-7C3F-4E58-9A21-VPNCLI000001}}
AppName={#MyAppName}
AppVersion={#MyAppVersion}
AppPublisher={#MyAppPublisher}
AppPublisherURL={#MyAppURL}
DefaultDirName={autopf}\VPN CLI
DisableProgramGroupPage=yes
; 注册服务需要管理员权限；加入系统 PATH 同理。
PrivilegesRequired=admin
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=..\..\dist
OutputBaseFilename=vpn-cli-setup-{#MyAppVersion}
Compression=lzma
SolidCompression=yes
ChangesEnvironment=yes
WizardStyle=modern

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "addtopath"; Description: "将 vpn-cli 加入系统 PATH"; GroupDescription: "环境"; Flags: checkedonce
Name: "installservice"; Description: "注册为 Windows 服务（vpn-cli daemon install）"; GroupDescription: "后台服务"; Flags: unchecked

[Files]
; 源路径相对本 .iss 文件所在目录（packaging\windows\）。
Source: "..\..\target\release\vpn-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
; WireGuard 官方 wintun.dll：Windows 上用户态隧道(tun crate)运行时加载。
; 构建前由 CI / 脚本放到 packaging\windows\wintun.dll（不入库）。
Source: "wintun.dll"; DestDir: "{app}"; Flags: ignoreversion

[Registry]
; 将安装目录追加到系统 PATH（Check 防止重复追加）。
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: expandsz; ValueName: "Path"; ValueData: "{olddata};{app}"; \
    Tasks: addtopath; Check: NeedsAddPath('{app}')
; 指向随包 wintun.dll 的绝对路径：daemon 以服务运行时用它显式 load，
; 不依赖工作目录默认搜索（wg_userspace.rs 读 VPN_WINTUN_PATH）。
Root: HKLM; Subkey: "SYSTEM\CurrentControlSet\Control\Session Manager\Environment"; \
    ValueType: string; ValueName: "VPN_WINTUN_PATH"; ValueData: "{app}\wintun.dll"; \
    Flags: uninsdeletevalue

[Run]
; 可选：安装后注册 Windows Service。复用 vpn-platform DaemonRuntime（sc.exe）。
Filename: "{app}\{#MyAppExeName}"; Parameters: "daemon install"; \
    Flags: runhidden waituntilterminated; Tasks: installservice; \
    StatusMsg: "正在注册 vpn-cli Windows 服务..."

[UninstallRun]
; 卸载前先注销服务（仅当之前注册过；失败忽略）。
Filename: "{app}\{#MyAppExeName}"; Parameters: "daemon uninstall"; \
    Flags: runhidden waituntilterminated; RunOnceId: "VpnCliDaemonUninstall"

[Code]
{ 判断 {app} 是否已在系统 PATH 中，避免重复追加。 }
function NeedsAddPath(Param: string): Boolean;
var
  OrigPath: string;
begin
  if not RegQueryStringValue(HKLM,
    'SYSTEM\CurrentControlSet\Control\Session Manager\Environment',
    'Path', OrigPath) then
  begin
    Result := True;
    exit;
  end;
  { 用分号包裹比较，避免子串误判。 }
  Result := Pos(';' + Uppercase(Param) + ';', ';' + Uppercase(OrigPath) + ';') = 0;
end;
