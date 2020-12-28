mod server;

use clap::Clap;

use std::path::Path;
use std::fs::canonicalize;

use std::io;
use termion::raw::IntoRawMode;
use tui::Terminal;
use tui::backend::TermionBackend;
use tui::widgets::{Block, Borders, List, ListItem};
use tui::layout::{Layout, Constraint, Direction};
use tui::text::{Span, Spans};
use termion::input::TermRead;
use termion::event::Key;
use termion::screen::AlternateScreen;

use std::collections::HashMap;

use std::sync::{Arc,Mutex};
use std::sync::atomic::{AtomicBool,Ordering};

use std::sync::mpsc;

use std::thread;
use std::time;

use crate::server::HttpConnection;

use std::net::SocketAddr;

use nix::unistd;

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

struct Connection {
    addr: SocketAddr,
    bytes_sent: usize,
    bytes_requested: usize
}

impl Connection {
    pub fn new(addr: SocketAddr) -> Connection {
        Connection {
            addr: addr,
            bytes_sent: 0,
            bytes_requested: 0
        }
    }

    pub fn update(&mut self, conn: &HttpConnection) {
        self.bytes_sent = conn.bytes_sent;
        self.bytes_requested = conn.bytes_requested;
    }
}

struct ConnectionSet {
    connections: HashMap<SocketAddr, Connection>,
}

impl ConnectionSet {
    pub fn new() -> ConnectionSet {
        ConnectionSet {
            connections: HashMap::<SocketAddr, Connection>::new(),
        }
    }

    pub fn update(&mut self, current_conns: &HashMap<i32, HttpConnection>) {
        let mut reindexed = HashMap::<SocketAddr, &HttpConnection>::new();
        for (_, conn) in current_conns {
            let peer_addr = match conn.stream.peer_addr() {
                Ok(addr) => addr,
                Err(_) => { continue; }
            };
            reindexed.insert(peer_addr, &conn);
        }

        let mut to_delete = Vec::<SocketAddr>::new();
        for (_, conn) in &self.connections {
            if !reindexed.contains_key(&conn.addr) {
                to_delete.push(conn.addr);
            }
        }

        for addr in to_delete {
            self.connections.remove(&addr);
        }

        for (addr, conn) in reindexed {
            self.connections.entry(addr)
                .or_insert(Connection::new(addr))
                .update(conn);
        }
    }
}

enum ControlEvent {
    Quit,
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

    let connection_set = Arc::new(Mutex::new(ConnectionSet::new()));
    let connection_set_needs_update = Arc::new(AtomicBool::new(false));

    let needs_update_clone = Arc::clone(&connection_set_needs_update);

    let (read_end, write_end) = match unistd::pipe() {
        Ok(tuple) => tuple,
        Err(_) => {
            eprintln!("Could not create pipe :(");
            return Ok(());
        }
    };

    let (tx, rx) = mpsc::channel();

    let connection_set_ptr = connection_set.clone();
    let thd = thread::spawn(move || {
        let _ = display(connection_set_ptr, rx, &needs_update_clone);
        let _ = unistd::write(write_end, "\0".as_bytes());
        let _ = unistd::close(write_end);
    });

    let keys = thread::spawn(move || {
        let stdin = io::stdin();
        for evt in stdin.keys() {
            if let Ok(key) = evt {
                match key {
                    Key::Char('q') => { let _ = tx.send(ControlEvent::Quit); break; },
                    _ => {}
                }
            }
        }
    });

    println!("Starting http server");

    tui.run(read_end, move |connections| {
        if connection_set_needs_update.swap(false, Ordering::Relaxed) {
            connection_set.lock().unwrap().update(&connections);
        }
    });

    let _ = unistd::close(read_end);

    println!("Http server is closing");

    let _ = thd.join();
    let _ = keys.join();

    println!("Display thd joined");

    Ok(())
}

fn build_str(addr: &SocketAddr, conn: &Connection) -> String {
    let ip_str = match addr {
        SocketAddr::V4(v4_addr) => {
            format!("{host}:{port} => {sent}/{reqd} ({perc}%)",
                    host=v4_addr.ip(), port=v4_addr.port(),
                    sent=conn.bytes_sent, reqd=conn.bytes_requested,
                    perc=(100. * (conn.bytes_sent as f64))/(conn.bytes_requested as f64))
        }
        SocketAddr::V6(v6_addr) => {
            format!("[{host}:{port}] => {sent}/{reqd} ({perc}%)",
                    host=v6_addr.ip(), port=v6_addr.port(),
                    sent=conn.bytes_sent, reqd=conn.bytes_requested,
                    perc=(100. * (conn.bytes_sent as f64))/(conn.bytes_requested as f64))
        }
    };
    
    ip_str
}

fn display(connection_set: Arc<Mutex<ConnectionSet>>, rx: mpsc::Receiver<ControlEvent>, needs_update: &AtomicBool) -> Result<(), io::Error> {

    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    'outer: loop {
        // if needs_update was false, it has been updated
        if !needs_update.swap(true, Ordering::Relaxed) {

            // Print that the connection has been established
            let conn_set = &connection_set.lock().unwrap().connections;
            let messages: Vec<ListItem> = conn_set.iter().map(|(addr, conn)| {
                ListItem::new(vec![Spans::from(Span::raw(build_str(addr, conn)))])
            }).collect();

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
                let block = List::new(messages).block(Block::default().borders(Borders::ALL).title("Connections"));
                f.render_widget(block, chunks[1]);
            })?;
        }

        loop {
            match rx.try_recv() {
                Ok(ControlEvent::Quit) => { break 'outer; },
                Err(mpsc::TryRecvError::Empty) => { break; }
                Err(mpsc::TryRecvError::Disconnected) => { break 'outer; }
            }
        }

        // If we don't chill a little, we'll actually slow down the http server
        // because we'll be doing a ton of copies.
        thread::sleep(time::Duration::from_millis(100));
    }

    Ok(())
}
