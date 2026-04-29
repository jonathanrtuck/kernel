//! Loom concurrency models of kernel protocols.
//!
//! These are ABSTRACT models — they replicate the kernel's concurrency
//! algorithms using loom::sync primitives, NOT testing the actual kernel
//! code (which is no_std and cannot run on the host). Each model extracts
//! the protocol from the corresponding kernel module and verifies it under
//! Loom's exhaustive interleaving exploration.
//!
//! | Model            | Kernel source       | Protocol verified                          |
//! |------------------|---------------------|--------------------------------------------|
//! | spinlock         | frame/lock.rs       | AtomicBool CAS mutual exclusion            |
//! | ipc_rendezvous   | communication.rs    | send/receive rendezvous delivery guarantee |

#[cfg(test)]
mod ipc_rendezvous;
#[cfg(test)]
mod spinlock;
