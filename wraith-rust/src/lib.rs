// Platform guard — fail fast with a clear message.
#[cfg(not(target_os = "linux"))]
compile_error!("wraith-capturer only supports Linux");

#[cfg(not(target_arch = "x86_64"))]
compile_error!("wraith-capturer only supports x86-64 (see phase8.md Phase 8.4 for cross-arch plans)");

pub mod capturer;
pub mod error;
pub mod ptrace_ops;
pub mod registers;
pub mod utils;
