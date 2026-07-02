fn main() {
    // Windows:嵌入 requireAdministrator 应用清单,使桌面端启动即弹 UAC 提权
    // (开 wintun 虚拟网卡、改写路由表需要管理员)。app_manifest 会替换 tauri 默认清单,
    // windows-app-manifest.xml 已是「默认清单 + requireAdministrator」的完整版。
    // 在非 Windows 平台,windows_attributes 为 no-op,不影响 macOS/Linux 构建。
    let attributes = tauri_build::Attributes::new().windows_attributes(
        tauri_build::WindowsAttributes::new()
            .app_manifest(include_str!("windows-app-manifest.xml")),
    );
    tauri_build::try_build(attributes).expect("tauri-build 失败");
}
