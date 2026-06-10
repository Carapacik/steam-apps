use std::process;

use steam_apps::{default_output_dir, generate_catalog};

fn main() {
    let output_dir = default_output_dir();
    if let Err(error) = generate_catalog(&output_dir) {
        eprintln!("{error:#}");
        process::exit(1);
    }
}
