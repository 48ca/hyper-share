#[macro_use] extern crate lazy_static;

use clap::Clap;

use std::path::{Path, Display};
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

mod server;
use server::{HttpConnection, HttpTui};

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

struct ConnectionSpeedMeasurement {
    speeds: [f32; 3],
    ind: usize,
}

impl ConnectionSpeedMeasurement {
    pub fn new() -> ConnectionSpeedMeasurement {
        return ConnectionSpeedMeasurement {
            speeds: [0., 0., 0.],
            ind: 0,
        }
    }

    pub fn update(&mut self, speed: f32) {
        self.speeds[self.ind] = speed;
        self.ind = (self.ind+1) % 3;
    }

    pub fn get_avg(&self) -> f32 {
        return (self.speeds[0] + self.speeds[1] + self.speeds[2])/3.;
    }
}

struct Connection {
    addr: SocketAddr,
    bytes_sent: usize,
    bytes_requested: usize,
    prev_bytes_sent: usize,
    update_time: time::Instant,
    prev_update_time: time::Instant,
    avg_speed: ConnectionSpeedMeasurement,
    last_requested_uri: String,
    num_requests: usize,
}

impl Connection {
    pub fn new(addr: SocketAddr) -> Connection {
        Connection {
            addr: addr,
            bytes_sent: 0,
            prev_bytes_sent: 0,
            bytes_requested: 0,
            update_time: time::Instant::now(),
            prev_update_time: time::Instant::now(),
            avg_speed: ConnectionSpeedMeasurement::new(),
            last_requested_uri: "[Reading...]".to_string(),
            num_requests: 0,
        }
    }

    pub fn update(&mut self, conn: &HttpConnection) -> bool {
        self.bytes_sent = conn.bytes_sent;
        self.bytes_requested = conn.bytes_requested;
        if let Some(uri) = &conn.last_requested_uri {
            if self.num_requests < conn.num_requests {
                self.last_requested_uri = uri.clone();
                self.num_requests = conn.num_requests;
                return true;
            }
        }
        false
    }

    pub fn estimated_speed(&mut self) -> f32 {
        self.prev_update_time = self.update_time;
        self.update_time = time::Instant::now();
        let dur = self.update_time.duration_since(self.prev_update_time);

        let millis: u64 = 1000 * dur.as_secs() + (dur.subsec_nanos() as u64)/1000000;
        if millis == 0 { return 0.; }
        let speed = (self.bytes_sent - self.prev_bytes_sent) as f32 / (millis as f32) * 1000.0;
        self.avg_speed.update(speed);

        self.prev_bytes_sent = self.bytes_sent;

        self.avg_speed.get_avg()
    }
}

struct History {
    history: Vec<Option<String>>,
    history_idx: usize,
}

impl History {
    pub fn new() -> History {
        History {
            history: vec![None; 20],
            history_idx: 0,
        }
    }

    pub fn push(&mut self, s: String) {
        self.history[self.history_idx] = Some(s);
        self.history_idx = (self.history_idx + 1) % 20;
    }

    pub fn iter<'a>(&'a self) -> HistoryIterator<'a> {
        HistoryIterator::new(self)
    }

    pub fn get_idx(&self) -> usize {
        if self.history_idx == 0 {
            self.capacity() - 1
        } else {
            self.history_idx - 1
        }
    }

    pub fn get(&self, i: usize) -> &Option<String> {
        &self.history[i]
    }

    pub fn capacity(&self) -> usize {
        self.history.len()
    }
}

struct HistoryIterator<'a> {
    data: &'a History,
    curr_idx: usize,
    start_idx: usize,
    done: bool,
}

impl HistoryIterator<'_> {
    pub fn new<'a>(hist: &'a History) -> HistoryIterator<'a> {
        HistoryIterator {
            data: hist,
            curr_idx: hist.get_idx(),
            start_idx: hist.get_idx(),
            done: false,
        }
    }
}

impl<'a> Iterator for HistoryIterator<'a> {
    type Item = &'a str;

    fn next(&mut self) -> Option<&'a str> {
        if self.done { return None; }

        let next_idx = if self.curr_idx == 0 {
            self.data.capacity() - 1
        } else {
            self.curr_idx - 1
        };

        if next_idx == self.start_idx {
            self.done = true;
        }

        if let Some(s) = self.data.get(self.curr_idx) {
            self.curr_idx = next_idx;

            return Some(&s);
        }

        None
    }
}

struct ConnectionSet {
    connections: HashMap<SocketAddr, Connection>,
    history: History,
}

impl ConnectionSet {
    pub fn new() -> ConnectionSet {
        ConnectionSet {
            connections: HashMap::<SocketAddr, Connection>::new(),
            history: History::new(),
        }
    }

    pub fn history(&self) -> &History {
        &self.history
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
            // `update()` returns true if there is a new request
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

    let (hist_tx, hist_rx) = mpsc::channel();

    let mut tui = match HttpTui::new(&opts.host, opts.port, &canon_path.as_path(), hist_tx) {
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
    let canon_path = canon_path.clone();
    let thd = thread::spawn(move || {
        let _ = display(canon_path.display(), connection_set_ptr, rx, &needs_update_clone);
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

    tui.run(read_end, move |connections| {
        if connection_set_needs_update.swap(false, Ordering::Relaxed) {
            let mut conn_set = connection_set.lock().unwrap();
            conn_set.update(&connections);
            loop {
                match hist_rx.try_recv() {
                    Ok(s) => { conn_set.history.push(s); }
                    Err(mpsc::TryRecvError::Empty) => { break; }
                    Err(mpsc::TryRecvError::Disconnected) => { break; }
                }
            }
        }
    });

    let _ = unistd::close(read_end);

    let _ = thd.join();
    let _ = keys.join();

    Ok(())
}

fn build_str(addr: &SocketAddr, conn: &mut Connection) -> String {
    let perc = if conn.bytes_requested == 0 { 0 } else {
        100 * conn.bytes_sent/conn.bytes_requested
    };
    let speed = conn.estimated_speed();
    let mut ip_str = match addr {
        SocketAddr::V4(v4_addr) => {
            format!("{host}:{port}", host=v4_addr.ip(), port=v4_addr.port())
        }
        SocketAddr::V6(v6_addr) => {
            format!("[{host}:{port}]", host=v6_addr.ip(), port=v6_addr.port())
        }
    };
    let info_str = format!(" {uri} #{num} => {sent}/{reqd}\t ({perc}% {speed} MiB/s)",
            uri=conn.last_requested_uri,
            num=conn.num_requests,
            sent=conn.bytes_sent, reqd=conn.bytes_requested,
            perc=perc,
            speed=speed / (1024. * 1024.));
    
    ip_str.push_str(&info_str);

    ip_str
}

fn display(root_path: Display, connection_set: Arc<Mutex<ConnectionSet>>, rx: mpsc::Receiver<ControlEvent>, needs_update: &AtomicBool) -> Result<(), io::Error> {

    let stdout = io::stdout().into_raw_mode()?;
    let stdout = AlternateScreen::from(stdout);
    let backend = TermionBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    'outer: loop {
        // if needs_update was false, it has been updated
        // if !needs_update.swap(true, Ordering::Relaxed) {
        if true {

            needs_update.store(true, Ordering::Relaxed);

            // Print that the connection has been established
            let conn_set = &mut connection_set.lock().unwrap();
            let conns = &mut conn_set.connections;

            let messages_connections: Vec<ListItem> = conns.iter_mut().map(|(addr, conn)| {
                ListItem::new(vec![Spans::from(Span::raw(build_str(addr, conn)))])
            }).collect();

            let messages_history: Vec<ListItem> = conn_set.history().iter().map(|s| {
                ListItem::new(vec![Spans::from(Span::raw(s))])
            }).collect();

            terminal.draw(|f| {
                let chunks = Layout::default()
                    .direction(Direction::Vertical)
                    .margin(1)
                    .constraints(
                        [
                            Constraint::Length(3),
                            Constraint::Min(2),
                            Constraint::Percentage(50),
                        ].as_ref()
                    )
                    .split(f.size());

                let block = List::new(
                                vec![ListItem::new(vec![Spans::from(Span::raw(format!("Serving {}", root_path)))])]
                            ).block(Block::default().borders(Borders::ALL).title("Information"));
                f.render_widget(block, chunks[0]);

                let block = List::new(messages_connections).block(Block::default().borders(Borders::ALL).title("Connections"));
                f.render_widget(block, chunks[1]);

                let block = List::new(messages_history).block(Block::default().borders(Borders::ALL).title("Request History"));
                f.render_widget(block, chunks[2]);
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
