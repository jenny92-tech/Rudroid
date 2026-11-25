//! INPUT:  process argv for the engine-agnostic CLI binary
//! OUTPUT:  ExitCode from shared CLI orchestration
//! POS:    thin `rudroid-loader` entry; logic lives in `cli`
#![deny(unsafe_op_in_unsafe_fn)]

#[path = "../cli.rs"]
mod cli;

rudroid_common::export_build_profile!();

fn main() -> std::process::ExitCode {
    cli::run_cli()
}
