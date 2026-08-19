//! Private direct-argv launcher that proves target `exec` before adapter completion.

#![forbid(unsafe_code)]

use std::{env, process};

fn main() {
    process::exit(tiber_process_linux::run_private_launcher(
        env::args_os().skip(1),
    ));
}
