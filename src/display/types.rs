use crate::http::HttpConnection;

use std::collections::HashMap;

use std::time;

use std::net::SocketAddr;

pub struct ConnectionSpeedMeasurement {
    speeds: [f32; 3],
    ind: usize,
}

impl ConnectionSpeedMeasurement {
    pub fn new() -> ConnectionSpeedMeasurement {
        return ConnectionSpeedMeasurement {
            speeds: [0., 0., 0.],
            ind: 0,
        };
    }

    pub fn update(&mut self, speed: f32) {
        self.speeds[self.ind] = speed;
        self.ind = (self.ind + 1) % 3;
    }

    pub fn get_avg(&self) -> f32 { return (self.speeds[0] + self.speeds[1] + self.speeds[2]) / 3.; }
}

pub struct Connection {
    pub addr: SocketAddr,
    pub bytes_sent: usize,
    pub bytes_requested: usize,
    pub bytes_read: usize,
    pub prev_bytes_sent: usize,
    pub update_time: time::Instant,
    pub prev_update_time: time::Instant,
    pub avg_speed: ConnectionSpeedMeasurement,
    pub last_requested_uri: String,
    pub num_requests: usize,
}

impl Connection {
    pub fn new(addr: SocketAddr) -> Connection {
        Connection {
            addr: addr,
            bytes_sent: 0,
            bytes_requested: 0,
            bytes_read: 0,
            prev_bytes_sent: 0,
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
        self.bytes_read = conn.bytes_read;
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

        let millis: u64 = 1000 * dur.as_secs() + (dur.subsec_nanos() as u64) / 1000000;
        if millis == 0 {
            return 0.;
        }
        let speed = (self.bytes_sent - self.prev_bytes_sent) as f32 / (millis as f32) * 1000.0;
        self.avg_speed.update(speed);

        self.prev_bytes_sent = self.bytes_sent;

        self.avg_speed.get_avg()
    }
}

pub struct History {
    history: Vec<Option<String>>,
    history_idx: usize,
}

impl History {
    pub fn new() -> History {
        History {
            history: vec![None; 50],
            history_idx: 0,
        }
    }

    pub fn push(&mut self, s: String) {
        self.history[self.history_idx] = Some(s);
        self.history_idx = (self.history_idx + 1) % 50;
    }

    pub fn iter<'a>(&'a self) -> HistoryIterator<'a> { HistoryIterator::new(self) }

    pub fn get_idx(&self) -> usize {
        if self.history_idx == 0 {
            self.capacity() - 1
        } else {
            self.history_idx - 1
        }
    }

    pub fn get(&self, i: usize) -> &Option<String> { &self.history[i] }

    pub fn capacity(&self) -> usize { self.history.len() }
}

pub struct HistoryIterator<'a> {
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
        if self.done {
            return None;
        }

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

pub struct ConnectionSet {
    pub connections: HashMap<SocketAddr, Connection>,
    pub history: History,
}

impl ConnectionSet {
    pub fn new() -> ConnectionSet {
        ConnectionSet {
            connections: HashMap::<SocketAddr, Connection>::new(),
            history: History::new(),
        }
    }

    pub fn update(&mut self, current_conns: &HashMap<i32, HttpConnection>) {
        let mut reindexed = HashMap::<SocketAddr, &HttpConnection>::new();
        for (_, conn) in current_conns {
            let peer_addr = match conn.stream.peer_addr() {
                Ok(addr) => addr,
                Err(_) => {
                    continue;
                }
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
            self.connections
                .entry(addr)
                .or_insert(Connection::new(addr))
                .update(conn);
        }
    }
}

pub enum ControlEvent {
    Quit,
    Toggle,
    CloseAll,
}
