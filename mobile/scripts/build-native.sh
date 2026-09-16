#!/usr/bin/env bash
set -euo pipefail
cd "$(dirname "$0")/../.."
: "${ANDROID_HOME:?Set ANDROID_HOME to the Android SDK}"
export ANDROID_NDK_HOME="${ANDROID_NDK_HOME:-$ANDROID_HOME/ndk/28.2.13676358}"
# cargo-ndk also aligns ELF LOAD segments for Android 16 KiB page devices.
cargo ndk --platform 26 -t arm64-v8a -t armeabi-v7a -t x86_64 -o target/mobile-jni build --locked -p vpn-mobile --release

# Package only our public JNI library. Cargo may also emit unused dependency cdylibs.
for abi in arm64-v8a armeabi-v7a x86_64; do
    mkdir -p "mobile/android/app/src/main/jniLibs/$abi"
    cp "target/mobile-jni/$abi/libvpn_mobile.so" "mobile/android/app/src/main/jniLibs/$abi/libvpn_mobile.so"
done
