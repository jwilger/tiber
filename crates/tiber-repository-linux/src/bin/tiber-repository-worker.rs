#![forbid(unsafe_code)]

/// Closed parent-worker protocol shared with the adapter library.
#[path = "../protocol.rs"]
mod protocol;
/// Private fixed repository syscall interpreter.
#[path = "../worker.rs"]
mod worker;

fn main() {
    if worker::run().is_err() {
        use std::process;
        process::exit(1);
    }
}
