use std::env;
use std::process;
use xpopup::run_popup;

fn main() {
    let args: Vec<String> = env::args().collect();

    // Strip --warn-only flag before positional args.
    let warn_only = args.iter().any(|a| a == "--warn-only");
    let positional: Vec<&String> = args.iter().skip(1).filter(|a| *a != "--warn-only").collect();

    if positional.is_empty() {
        eprintln!("Usage: xpopup [--warn-only] <message> [background_image_path]");
        process::exit(1);
    }

    let message = positional[0].clone();
    let bg_image_path = positional.get(1).map(|s| (*s).clone());

    run_popup(message, bg_image_path, warn_only);
}
