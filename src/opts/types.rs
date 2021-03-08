use clap::Clap;

#[derive(Clap, Clone)]
#[clap(version = "0.2.1", author = "James Houghton <jamesthoughton@gmail.com")]
pub struct Opts {
    #[clap(short, long, default_value = ".")]
    pub directory: String,
    #[clap(short, long, default_value = "80")]
    pub port: u16,
    #[clap(short = 'm', long, default_value = "0.0.0.0")]
    pub hostmask: String,
    #[clap(short, long = "upload", about = "Enable uploading capabilities")]
    pub uploading_enabled: bool,
    #[clap(long = "nodirs", about = "Disable directory listings")]
    pub disable_directory_listings: bool,
    #[clap(
        long = "start-disabled",
        about = "Start the server as disabled. Files will not be served until the server is \
                 enabled."
    )]
    pub start_disabled: bool,
    #[clap(
        short = 'r',
        long = "ui-refresh-rate",
        default_value = "100",
        about = "In milliseconds, how often the UI will be updated"
    )]
    pub ui_refresh_rate: u64,
    #[clap(long, about = "Do not start the interface (useful for testing)")]
    pub headless: bool,
    #[clap(
        long = "upload-size-limit",
        about = "Uploaded file size limit in bytes. Specify 0 for no limit.",
        default_value = "0"
    )]
    pub size_limit: usize,
    #[clap(
        long = "index-file",
        about = "Index page filename. When rendering a directory, render this file instead.",
        default_value = "index.html"
    )]
    pub index_file: String,
    #[clap(
        long = "no-index-file",
        about = "Disable the index file. Always render directories."
    )]
    pub no_index_file: bool,
    #[clap(
        long = "no-slash",
        about = "When navigating to a directory, hypershare will not try to append a '/' to the \
                 path."
    )]
    pub no_append_slash: bool,
}
