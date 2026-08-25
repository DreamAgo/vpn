// Prevents an extra console window on Windows in release. Harmless on macOS.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if let Some(code) = vpn_desktop_lib::run_macos_helper_if_requested() {
        std::process::exit(code);
    }
    vpn_desktop_lib::run();
}
