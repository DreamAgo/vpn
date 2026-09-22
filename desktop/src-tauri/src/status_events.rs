//! State-change invalidation; clients fetch authoritative snapshots after a notification.
use tauri::Emitter;

const EVENT: &str = "vpn-status-changed";

pub fn start(app: tauri::AppHandle) {
    tauri::async_runtime::spawn(async move {
        #[cfg(not(target_os = "macos"))]
        {
            use std::sync::Arc;
            use tauri::Manager;
            let manager = app.state::<Arc<crate::manager::VpnManager>>();
            let mut changes = manager.subscribe_status();
            while changes.changed().await.is_ok() {
                let _ = app.emit_to("main", EVENT, ());
            }
        }
        #[cfg(target_os = "macos")]
        {
            let mut previous = vpn_cli::ipc::StatusResponse::disconnected();
            loop {
                match crate::macos_helper::wait_status(previous.clone()).await {
                    Ok(status) => {
                        if !status.same_connection(&previous) {
                            let _ = app.emit_to("main", EVENT, ());
                        }
                        previous = status;
                    }
                    Err(_) => {
                        // No helper / previous version / session switch: polling remains
                        // authoritative and retries here must not spin or trigger install.
                        tokio::time::sleep(std::time::Duration::from_millis(2500)).await;
                    }
                }
            }
        }
    });
}
