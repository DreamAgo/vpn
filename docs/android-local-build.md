# Android 本地构建环境

本机为 Apple Silicon macOS。以下工具已于 2026-09-16 安装；Android 客户端工程已实现，功能与验证记录见 [mobile.md](mobile.md)。

| 工具 | 版本 / 位置 |
| --- | --- |
| JDK | Homebrew OpenJDK 17，`/opt/homebrew/opt/openjdk@17` |
| Android SDK | `$HOME/Library/Android/sdk` |
| SDK Platform / Build Tools | API 35、36 / 35.0.0、36.0.0 |
| NDK | 28.2.13676358（r28c） |
| Rust | 项目固定 1.90；Android ARM64、ARMv7、x86_64 标准库 |
| cargo-ndk | 4.1.2 |
| Gradle | 8.11.1，`$HOME/Library/Android/gradle/gradle-8.11.1` |

环境变量统一保存在 `$HOME/Library/Android/env.sh`，由 `.zprofile` 和 `.zshrc` 加载。
原有两个配置文件各保留一份 `.before-android-<时间>` 备份。
已经打开的终端可执行：

```sh
source "$HOME/Library/Android/env.sh"
java -version
sdkmanager --sdk_root="$ANDROID_HOME" --list_installed
adb version
cargo ndk --version
gradle --version
rustup target list --toolchain 1.90 --installed
```

Rust 原生库由 `mobile/scripts/build-native.sh` 构建。也可在项目根目录执行：

```sh
cargo +1.90 ndk \
  -t arm64-v8a -t armeabi-v7a -t x86_64 \
  --platform 26 -o target/mobile-jni build --locked -p vpn-mobile --release
```

Android 工程已提交 Gradle Wrapper 并固定 Android Gradle Plugin 和 NDK 版本，
使用 `mobile/android/gradlew -p mobile/android assembleDebug` 构建。
SDK 编译版本不等于最低系统版本；客户端最低支持 Android API 26。

本机安装的是命令行构建环境，未安装 Android Studio、模拟器及系统镜像。
APK 打包成功也不能替代 VPN 真机验证：需要单独检查授权内网通信、DNS、锁屏、
网络切换和断开后的系统网络恢复。

## 工具链验证记录

使用 `/private/tmp/vpn-android-toolchain-check` 临时工程验证，测试代码不属于 VPN 客户端。

- 新建登录 shell 能正确加载 JDK 17、SDK 和 Gradle。
- Rust 1.90 已把 `boringtun 0.6` 和本仓库 `vpn-obfs` 依赖编译为 ARM64、ARMv7、
  x86_64 三种 Android 动态库；检查产物确实为对应架构的 ELF。
- ARM64 测试动态库的 LOAD 段对齐为 `0x4000`（16 KiB）。
- Gradle 8.11.1 + Android Gradle Plugin 8.9.2 执行 `:app:assembleDebug` 成功，
  33 个任务通过；生成最低 API 26、目标 API 35 的测试 APK，包含上述三种架构原生库。
- 测试 APK 通过 `apksigner verify`（v2 签名）与 `zipalign -c -P 16 4` 检查。
- 未连接真机、未执行 APK 安装或 VPN 连通测试。临时 APK 仅验证工具链，不是 VPN 客户端。
