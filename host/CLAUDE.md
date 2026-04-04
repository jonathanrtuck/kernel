# host crate

Host-side tests that run natively (not on bare metal). Used to verify kernel logic that doesn't depend on hardware — data structures, algorithms, state machines.

## Run tests

```sh
cargo test
```

## Purpose

Extract testable kernel logic and verify it on the host where debugging is easy. Anything that doesn't touch MMU registers, exception vectors, or privilege boundaries can be tested here.
