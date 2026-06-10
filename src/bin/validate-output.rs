use std::process;

use steam_apps::{default_output_dir, validate_output_dir};

fn main() {
    let output_dir = default_output_dir();
    if let Err(error) = validate_output_dir(&output_dir) {
        eprintln!("{error:#}");
        process::exit(1);
    }
}
