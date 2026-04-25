//! Microkernel library.
//!
//! Module root for the kernel crate. Declares the module tree used by both
//! the bare-metal binary (`main.rs`) and host-side test builds
//! (`cargo test --target aarch64-apple-darwin`).

#![no_std]
#![deny(unsafe_code)]

#[allow(unsafe_code)]
pub mod frame;

pub mod arena;
pub mod capability;
pub mod config;
pub mod fault;
pub mod field;
pub mod observer;
#[cfg(any(target_os = "none", test))]
pub mod print;
pub mod pulsar;
pub mod scheduler;
pub mod space;
pub mod syscall;
pub mod time;
