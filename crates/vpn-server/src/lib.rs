//! vpn-server library 入口（main.rs 仅做启动调度）。

pub mod app;
pub mod config;
pub mod error;
pub mod handlers;
pub mod middleware;
pub mod shutdown;
pub mod startup;
pub mod state;
pub mod tls;

pub use app::build_router;
pub use config::ServerConfig;
pub use state::AppState;
