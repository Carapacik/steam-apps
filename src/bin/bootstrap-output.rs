use std::process;

use steam_apps::{default_output_dir, write_bootstrap_files};

fn main() {
    let output_dir = default_output_dir();
    if let Err(error) = write_bootstrap_files(&output_dir) {
        eprintln!("{error:#}");
        process::exit(1);
    }
}
