use std::env;
use std::process;
use xpopup::run_popup;

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 2 {
        eprintln!("Usage: xpopup <message> [background_image_path]");
        process::exit(1);
    }

    let message = args[1].clone();
    let bg_image_path = args.get(2).cloned();

    run_popup(message, bg_image_path);
}
