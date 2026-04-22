//! Pulsar: capability-held timer object with kernel-managed delivery.
//!
//! D44: fifth kernel object type.
//! Created from Space (D32), delivers to a Field (D13/D17).
//! Kernel manages re-arm, drift compensation, overflow.
//! Period is EDF admission input (D42).
