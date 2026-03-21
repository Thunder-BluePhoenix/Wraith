// Platform guard — fail fast with a clear message.
#[cfg(not(target_os = "linux"))]
compile_error!("wraith-capturer only supports Linux");

#[cfg(not(target_arch = "x86_64"))]
compile_error!("wraith-capturer only supports x86-64 (see phase8.md Phase 8.4 for cross-arch plans)");

// Phase 1
pub mod capturer;
pub mod error;
pub mod ptrace_ops;
pub mod registers;
pub mod utils;

// Phase 2
pub mod fd_enum;
pub mod memory;
pub mod proto;
pub mod snapshot;

// Phase 4
pub mod aslr;
pub mod fd_restore;
pub mod restorer;
