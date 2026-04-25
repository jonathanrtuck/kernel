//! Framekernel core — the unsafe boundary.
//!
//! All `unsafe` code in the kernel lives inside this module tree. Everything
//! outside `frame` is safe Rust built against the abstractions exported here.
//! The crate-level `#![deny(unsafe_code)]` enforces this at compile time.

#[cfg(any(target_os = "none", test))]
pub mod arch;
pub mod cap_ops;
pub mod core_ops;
pub mod field_ops;
pub mod firmware;
pub mod lock;
pub mod slab;
