mod server;

use clap::Clap;

use std::path::Path;
use std::fs::canonicalize;

#[derive(Clap)]
#[clap(version="1.0", author="James Houghton <jhoughton@virginia.edu>")]
struct Opts {
    #[clap(short, long, default_value = ".")]
    directory: String,
    #[clap(short, long, default_value = "80")]
    port: u16,
    #[clap(short, long, default_value = "127.0.0.1")]
    host: String,
}

fn main() {
    let opts: Opts = Opts::parse();
    let path = Path::new(&opts.directory);
    let canon_path = canonicalize(path).unwrap();

    println!("Got path: {}", canon_path.display());
    let mut tui = server::HttpTui::new(&opts.host, opts.port, &canon_path.as_path());
    tui.run(|| {
        println!("Update function");

        // None: keep the server going
        None
    });
}
