//! Portable packet engine. Platform code owns sockets, TUN, TLS and credentials.
//! Engines are synchronous; callers serialize operations and close platform IO
//! before dropping an engine. No background threads or retained caller buffers.
pub mod engine;
#[cfg(any(target_os = "android", test))]
mod ffi;
pub mod policy;
pub mod routes;
pub mod session;

pub type Result<T> = std::result::Result<T, String>;
