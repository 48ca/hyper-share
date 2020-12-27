mod server;

use clap::Clap;

use std::path::Path;
use std::fs::canonicalize;

use std::io;
use termion::raw::IntoRawMode;
use tui::Terminal;
use tui::backend::TermionBackend;
use tui::widgets::{Widget, Block, Borders};
use tui::layout::{Layout, Constraint, Direction};

use std::thread;
use std::sync::mpsc;

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

fn main() -> Result<(), io::Error> {
    let opts: Opts = Opts::parse();
    let path = Path::new(&opts.directory);
    let canon_path = match canonicalize(path) {
        Ok(path) => path,
        Err(e) => {
            eprintln!("Failed to open directory {}: {}", opts.directory, e);
            return Ok(())
        }
    };
    let mut tui = match server::HttpTui::new(&opts.host, opts.port, &canon_path.as_path()) {
        Ok(tui) => tui,
        Err(e) => {
            eprintln!("Failed to bind to port {}: {}", opts.port, e);
            return Ok(());
        }
    };

    // Does not return until HttpTuiOperation::Kill is sent here
    tui.run(|connections| {
        println!("Got new connections");
        for (fd, conn) in connections {
            println!("Connection {}: {}/{}", fd, conn.bytes_sent, conn.bytes_requested);
            if let Some(response) = &conn.response {
                println!("Connection {}: chunk size: {}", fd, response.chunk_size());
            }
        }

        // None: keep the server going
        None
    });

    Ok(())
}

#[allow(dead_code)]
fn display() -> Result<(), io::Error> {
    let stdout = io::stdout().into_raw_mode()?;
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    loop {
        terminal.draw(|f| {
            let chunks = Layout::default()
                .direction(Direction::Vertical)
                .margin(1)
                .constraints(
                    [
                        Constraint::Percentage(10),
                        Constraint::Percentage(80),
                        Constraint::Percentage(10)
                    ].as_ref()
                )
                .split(f.size());
            let block = Block::default()
                 .title("Block")
                 .borders(Borders::ALL);
            f.render_widget(block, chunks[0]);
            let block = Block::default()
                 .title("Block 2")
                 .borders(Borders::ALL);
            f.render_widget(block, chunks[1]);
        })?;
    }
}
