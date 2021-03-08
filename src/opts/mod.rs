pub mod types;

use std::process;

pub fn verify_opts(opts: &types::Opts) {
    if opts.start_disabled && opts.headless {
        println!(
            "Warning: --start-disabled and --headless have both been specified. The server will \
             remain disabled, as there is no way to enable it while running headless."
        );
    }

    if opts.index_file.contains("/") {
        println!("Error: invalid index file.");
        process::exit(1);
    }
}
