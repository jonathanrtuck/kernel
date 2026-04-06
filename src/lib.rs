//! Microkernel library.
//!
//! Module root for the kernel crate. Declares the module tree used by both
//! the bare-metal binary (`main.rs`) and host-side test builds
//! (`cargo test --target aarch64-apple-darwin`).

#![no_std]

#[cfg(target_os = "none")]
pub mod arch;
pub mod config;
pub mod firmware;
#[cfg(target_os = "none")]
pub mod print;
