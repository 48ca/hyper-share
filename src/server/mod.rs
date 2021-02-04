mod http_core;
mod rendering;

use nix::unistd;

use std::fs::OpenOptions;

use std::ptr::copy;

use std::path::PathBuf;

use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;

use std::cmp::{max, min};

use std::str::from_utf8;

use std::io::{self, Read, Seek, Write};

use std::fs;

use std::format;

use std::collections::HashMap;

use nix::sys::select::{select, FdSet};
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;

use std::sync::mpsc;

use std::path::Path;

use http_core::types::{ResponseDataType, SeekableString};
use http_core::{HttpMethod, HttpRequest, HttpResponse, HttpStatus, HttpVersion};

extern crate boyer_moore_magiclen;

use core::slice::Iter;

use boyer_moore_magiclen::{BMByte, BMByteSearchable};

const BUFFER_SIZE: usize = 4096;
const POST_BUFFER_SIZE: usize = 4096 * 1024;

fn resolve_io_error(error: &io::Error) -> Option<HttpStatus> {
    match error.kind() {
        io::ErrorKind::NotFound => Some(HttpStatus::NotFound),
        io::ErrorKind::PermissionDenied => Some(HttpStatus::PermissionDenied),
        _ => None,
    }
}

struct ContentRange {
    pub start: usize,
    pub len: Option<usize>,
}

fn decode_content_range(range_str: &str) -> Option<ContentRange> {
    if !range_str.starts_with("bytes=") {
        return None;
    }
    let eq_ind = match range_str.find('=') {
        Some(i) => i,
        _ => {
            return None;
        }
    };
    let dash_ind = match range_str.find('-') {
        Some(i) => i,
        _ => {
            return None;
        }
    };

    let start_str = &range_str[eq_ind + 1..dash_ind];
    let end_str = &range_str[dash_ind + 1..];

    let start_int: usize = if start_str.len() > 0 {
        match start_str.parse() {
            Ok(i) => i,
            _ => {
                return None;
            }
        }
    } else {
        0
    };

    let end_int: Option<usize> = if end_str.len() > 0 {
        match end_str.parse() {
            Ok(i) => Some(i),
            _ => {
                return None;
            }
        }
    } else {
        None
    };

    if let Some(end_i) = end_int {
        if end_i == 0 || start_int > end_i {
            None
        } else {
            Some(ContentRange {
                start: start_int,
                len: Some(1 + end_i - start_int),
            })
        }
    } else {
        Some(ContentRange {
            start: start_int,
            len: None,
        })
    }
}

fn decode_request(req_body: &[u8]) -> Result<HttpRequest, HttpStatus> {
    let request_str = match from_utf8(req_body) {
        Ok(dec) => dec,
        Err(_err) => {
            // write_error(format!("Could not decode request: {}", err));
            return Err(HttpStatus::BadRequest);
        }
    };

    return HttpRequest::new(request_str);
}

fn end_of_http_request(req_body: &[u8]) -> bool {
    if req_body.len() < 4 {
        return false;
    }
    return &req_body[req_body.len() - 4..] == b"\r\n\r\n";
}

struct BMBuf<'a>(&'a [u8]);

impl BMByteSearchable for BMBuf<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn value_at(&self, index: usize) -> u8 {
        self.0[index]
    }

    #[inline]
    fn iter(&self) -> Iter<u8> {
        self.0.iter()
    }
}

#[derive(PartialEq, Debug)]
pub enum ConnectionState {
    ReadingRequest,
    ReadingPostBody,
    WritingResponse,
    Closing,
}

pub enum PostRequestState {
    AwaitingFirstBody,
    AwaitingBody,
    AwaitingMeta,
}

pub struct PostBuffer {
    pub fill_location: usize,
    pub buffer: Box<[u8; POST_BUFFER_SIZE]>,
    pub post_delimeter: BMByte,
    pub post_delimeter_string: String,
    pub current_file: Option<fs::File>,
    pub state: PostRequestState,
    pub dir: PathBuf,
    pub parse_idx: usize,
}

impl PostBuffer {
    pub fn new(dir: PathBuf, delim: BMByte, delim_str: String) -> PostBuffer {
        PostBuffer {
            buffer: Box::new([0; POST_BUFFER_SIZE]),
            fill_location: 0,
            post_delimeter: delim,
            post_delimeter_string: delim_str,
            current_file: None,
            state: PostRequestState::AwaitingFirstBody,
            dir: dir,
            parse_idx: 0,
        }
    }

    fn find_next_delim(&self, start: usize) -> Option<usize> {
        let vec = self
            .post_delimeter
            .find_in(BMBuf(&self.buffer[start..self.fill_location]), 1);
        if vec.len() < 1 {
            None
        } else {
            Some(vec[0] + start)
        }
    }

    fn send_buffer_data_to_file(&mut self, limit: usize) -> Result<(), String> {
        if self.current_file.is_none() {
            return Err("Attempted to write to a file before opening it.".to_string());
        }

        if limit < self.post_delimeter_string.len() {
            return Err("Not enough data to write anything.".to_string());
        }
        let real_limit: usize = limit - self.post_delimeter_string.len();
        let written = match self
            .current_file
            .as_ref()
            .unwrap()
            .write(&self.buffer[self.parse_idx..real_limit])
        {
            Ok(size) => size,
            Err(_) => {
                return Err("Error writing to file.".to_string());
            }
        };

        self.parse_idx += written;

        let amount_remaining: usize = self.fill_location - self.parse_idx;

        unsafe {
            // Shuffle
            if amount_remaining > self.parse_idx {
                panic!("About to do a ptr::copy call on aliased memory locations.");
            }
            copy(
                &self.buffer[self.parse_idx..].as_ptr(),
                &mut self.buffer[..].as_ptr(),
                amount_remaining,
            );

            /*
            // A safe version (if this copy could never alias) would be:
            &self.buffer[..amount_remaining]
                .clone_from_slice(&self.buffer[self.parse_idx..self.fill_location]);
            */
        }

        self.parse_idx = 0;
        self.fill_location = amount_remaining;

        Ok(())
    }

    pub fn handle_new_data(&mut self) -> Result<bool, String> {
        // Where parsing should begin
        loop {
            match self.state {
                PostRequestState::AwaitingFirstBody => {
                    self.parse_idx = match self.find_next_delim(self.parse_idx) {
                        None => {
                            // Cannot find the delimeter, so keep reading. This is good
                            // for slow connections. If we can't find the delimeter in 4M
                            // eventually `read` will return 0 and the connection will be
                            // aborted.
                            return Ok(false);
                        }
                        Some(idx) => idx + self.post_delimeter_string.len() + 2, // + 2 to account for following "\r\n"
                    };

                    let body_start =
                        match find_body_start(&self.buffer[self.parse_idx..self.fill_location]) {
                            Some(idx) => idx + self.parse_idx,
                            None => {
                                self.state = PostRequestState::AwaitingMeta;
                                return Ok(false);
                            }
                        };

                    let meta = &self.buffer[self.parse_idx..body_start];
                    let meta_str = String::from_utf8_lossy(meta).to_string();

                    let mut info: &str = "";

                    for line in meta_str.split("\r\n") {
                        let (head, val) = line.split_at(match meta_str.find(":") {
                            Some(idx) => idx,
                            None => {
                                return Err("Could not find ':' in Content-Disposition".to_string());
                            }
                        });
                        if head.to_lowercase() == "content-disposition" {
                            info = val;
                            break;
                        }
                    }
                    if info == "" {
                        return Err(format!(
                            "Did not receive a Content-Disposition:\n{}",
                            meta_str
                        ));
                    }

                    let mut filename: &str = "";
                    for kv in info.split(";") {
                        if let Some(idx) = kv.find("=") {
                            let (k, v) = kv.split_at(idx);
                            if k.trim_start() == "filename" {
                                // 1.. to discard '='
                                filename = &v[1..].trim();
                                break;
                            }
                        }
                    }

                    if filename == "" {
                        return Err("Could not attribute with a filename".to_string());
                    }

                    if filename.contains("/") {
                        return Err(format!("Invalid filename: {}", filename));
                    }

                    if filename.starts_with("\"") {
                        filename = &filename[1..filename.len() - 1];
                    }

                    let real_filename = self.dir.join(filename);

                    self.current_file = Some(
                        match OpenOptions::new()
                            .write(true)
                            .create_new(true)
                            .open(real_filename)
                        {
                            Ok(f) => f,
                            _ => {
                                return Err("Could not open file for writing.".to_string());
                            }
                        },
                    );

                    self.state = PostRequestState::AwaitingBody;

                    self.parse_idx = body_start;
                }
                PostRequestState::AwaitingBody => {
                    let end = match self.find_next_delim(self.parse_idx) {
                        None => {
                            self.send_buffer_data_to_file(self.fill_location)?;
                            return Ok(false);
                        }
                        Some(idx) => idx - 2, // - 2 to remove the preceding "\r\n"
                    };

                    // self.write_to_file(self.parse_idx);

                    return Ok(true);
                }
                // TODO: Test this.
                // AwaitingMeta will only happen if Content-Disposition
                // happens to land on a 4M boundary, very rare.
                PostRequestState::AwaitingMeta => {
                    panic!("AwaitingMeta not implemented");
                }
            }
        }
    }
}

pub struct HttpConnection {
    pub stream: TcpStream,
    pub state: ConnectionState,

    // Buffer for holding a pending request
    pub buffer: Box<[u8; BUFFER_SIZE]>,
    pub bytes_read: usize,
    pub body_start_location: usize,

    pub post_buffer: Option<PostBuffer>,

    // Space to store a per-request string response
    pub response: Option<HttpResponse>,

    pub last_requested_method: Option<HttpMethod>,
    pub last_requested_uri: Option<String>,
    pub num_requests: usize,

    pub keep_alive: bool,

    pub bytes_requested: usize,
    pub bytes_sent: usize,
}

impl HttpConnection {
    pub fn new(stream: TcpStream) -> HttpConnection {
        return HttpConnection {
            stream: stream,
            state: ConnectionState::ReadingRequest,
            buffer: Box::new([0; BUFFER_SIZE]),
            bytes_read: 0,
            body_start_location: 0,
            post_buffer: None,
            response: None,
            keep_alive: true,
            bytes_requested: 0,
            bytes_sent: 0,
            last_requested_uri: None,
            last_requested_method: None,
            num_requests: 0,
        };
    }

    pub fn reset(&mut self) {
        self.bytes_read = 0;
        self.response = None;
    }
}

enum HttpResult {
    Response(HttpResponse, usize),
    Error(HttpStatus, Option<String>),
    ReadRequestBody,
}

pub struct HttpTui<'a> {
    listener: TcpListener,
    root_dir: &'a Path,
    history_channel: mpsc::Sender<String>,
    dir_listings: bool,
    disabled: bool,
}

impl HttpTui<'_> {
    pub fn new<'a>(
        host: &str,
        port: u16,
        root_dir: &'a Path,
        sender: mpsc::Sender<String>,
        dir_listings: bool,
        disabled: bool,
    ) -> Result<HttpTui<'a>, io::Error> {
        let listener = TcpListener::bind(format!("{mask}:{port}", mask = host, port = port))?;
        Ok(HttpTui {
            listener: listener,
            root_dir: root_dir,
            history_channel: sender,
            dir_listings: dir_listings,
            disabled: disabled,
        })
    }

    pub fn run(&mut self, pipe_read: RawFd, func: impl Fn(&HashMap<RawFd, HttpConnection>)) {
        let mut connections = HashMap::<RawFd, HttpConnection>::new();
        let l_raw_fd = self.listener.as_raw_fd();

        'main: loop {
            let mut r_fds = FdSet::new();
            let mut w_fds = FdSet::new();
            let mut e_fds = FdSet::new();

            // First add listener:
            r_fds.insert(l_raw_fd);
            e_fds.insert(l_raw_fd);

            r_fds.insert(pipe_read);
            e_fds.insert(pipe_read);

            for (fd, http_conn) in &connections {
                match http_conn.state {
                    ConnectionState::WritingResponse => {
                        w_fds.insert(*fd);
                    }
                    ConnectionState::ReadingRequest => {
                        r_fds.insert(*fd);
                    }
                    ConnectionState::ReadingPostBody => {
                        r_fds.insert(*fd);
                    }
                    _ => {}
                }
                e_fds.insert(*fd);
            }

            match select(
                None,
                Some(&mut r_fds),
                Some(&mut w_fds),
                Some(&mut e_fds),
                None,
            ) {
                Ok(_res) => {}
                Err(e) => {
                    println!("Got error while selecting: {}", e);
                    break;
                }
            }

            match r_fds.highest() {
                None => {}
                Some(mfd) => {
                    for fd in 0..(mfd + 1) {
                        if !r_fds.contains(fd) {
                            continue;
                        }
                        // if !connections.contains_key(&fd) { continue; }

                        // If we have data to read on the pipe
                        if fd == pipe_read {
                            let mut buf: [u8; 1] = [0; 1];
                            if let Ok(size) = unistd::read(pipe_read, &mut buf[..]) {
                                if size == 0 {
                                    break 'main;
                                }
                                if buf[0] as char == 't' {
                                    self.disabled = !self.disabled;
                                }
                                if buf[0] as char == 'p' {
                                    // Poked :)
                                    // This is used to trigger another call
                                    // to `func`.
                                }
                                continue;
                            } else {
                                break 'main;
                            }
                        }
                        if fd == l_raw_fd {
                            // If listener, get accept new connection and add it.
                            if let Ok((stream, _addr)) = self.listener.accept() {
                                let conn = HttpTui::create_http_connection(stream);
                                let pfd = conn.stream.as_raw_fd();
                                connections.insert(pfd, conn);
                            }
                            // We cannot pass this new connection to handle_conn immediately,
                            // as we don't know if there is any data for us to read yet.
                            continue;
                        }
                        // TODO: Error checking here
                        let mut conn = connections.get_mut(&fd).unwrap();
                        match self.handle_conn_sigpipe(&mut conn) {
                            Ok(_) => {}
                            Err(error) => {
                                let _ = self.history_channel.send(format!(
                                    "Uncaught OS error while handling connection: {}",
                                    error
                                ));
                                // write_error(format!("Server error while reading: {}", error));
                            }
                        };
                    }
                }
            }
            match w_fds.highest() {
                None => {}
                Some(mfd) => {
                    for fd in 0..(mfd + 1) {
                        if !w_fds.contains(fd) {
                            continue;
                        }
                        // if !connections.contains_key(&fd) { continue; }
                        assert_eq!(connections[&fd].state, ConnectionState::WritingResponse);
                        match self.handle_conn_sigpipe(&mut connections.get_mut(&fd).unwrap()) {
                            Ok(_) => {}
                            _ => {} // Err(error) => { write_error(format!("Server error while writing: {}", error)); }
                        }
                    }
                }
            }
            match e_fds.highest() {
                None => {}
                Some(mfd) => {
                    for fd in 0..(mfd + 1) {
                        if !e_fds.contains(fd) {
                            continue;
                        }
                        // if !connections.contains_key(&fd) { continue; }
                        if fd == pipe_read {
                            break 'main;
                        }
                        // If listener, get accept new connection and add it.
                        if fd == l_raw_fd {
                            eprintln!("Listener socket has errored!");
                            break 'main;
                        } else {
                            println!("Got bad state on client socket");
                            connections.remove(&fd);
                        }
                    }
                }
            }

            let to_remove: Vec<_> = connections
                .iter()
                .filter(|&(_, conn)| conn.state == ConnectionState::Closing)
                .map(|(k, _)| k.clone())
                .collect();
            for fd in to_remove {
                connections.remove(&fd);
            }
            func(&connections);
        }
    }

    fn write_conn_to_history(&self, conn: &mut HttpConnection) {
        if let Ok(peer_addr) = conn.stream.peer_addr() {
            let ip_str = match peer_addr {
                SocketAddr::V4(addr) => format!("{}:{}", addr.ip(), addr.port()),
                SocketAddr::V6(addr) => format!("[{}]:{}", addr.ip(), addr.port()),
            };
            let code_str = match &conn.response {
                Some(resp) => resp.get_code(),
                None => "   ".to_string(),
            };
            let path_str = match &conn.last_requested_uri {
                Some(path) => path,
                None => "[No path...]",
            };
            let method_str = match &conn.last_requested_method {
                Some(HttpMethod::GET) => "GET",
                Some(HttpMethod::HEAD) => "HEAD",
                Some(HttpMethod::POST) => "POST",
                None => "???",
            };
            let _ = self.history_channel.send(format!(
                "{:<22} {} {:<4} {}",
                ip_str, code_str, method_str, path_str
            ));
        }
    }

    fn handle_request(&self, conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
        let res = self.parse_and_service_request(conn);
        self.write_conn_to_history(conn);

        let state = match res {
            Ok(state) => state,
            Err(error) => {
                match self.create_oneoff_response(
                    HttpStatus::ServerError,
                    conn,
                    Some(error.to_string()),
                ) {
                    Ok(state) => state,
                    Err(e) => return Err(e),
                }
            }
        };

        if state == ConnectionState::WritingResponse {
            // Force an initial write of the data
            self.write_partial_response(conn)
        } else {
            Ok(state)
        }
    }

    fn read_partial_request(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        let buffer = &mut conn.buffer;
        let bytes_read = match conn.stream.read(&mut buffer[conn.bytes_read..]) {
            Ok(size) => size,
            Err(_err) => {
                /*
                write_error(format!(
                    "Failed to read bytes from socket: {}", err));
                */
                // Even though the server has run into a problem, because it is
                // a problem inherent to the socket connection, we return Ok
                // so that we do not write an HTTP error response to the socket.
                return Ok(ConnectionState::Closing);
            }
        };

        conn.bytes_read += bytes_read;
        if end_of_http_request(&buffer[..conn.bytes_read]) {
            conn.body_start_location = conn.bytes_read;
            // Once we have read the request, handle it.
            // The connection state will be updated accordingly
            self.handle_request(conn)
        } else if bytes_read == 0 {
            return Ok(ConnectionState::Closing);
        } else if conn.bytes_read == buffer.len() {
            if let Some(start) = find_body_start(&conn.buffer[..conn.bytes_read]) {
                conn.body_start_location = start;
                return self.handle_request(conn);
            }
            return self.create_oneoff_response(
                HttpStatus::RequestHeadersTooLarge,
                conn,
                Some(
                    "Request headers are too long. The total size must be less than 4KB."
                        .to_string(),
                ),
            );
        } else {
            if let Some(start) = find_body_start(&conn.buffer[..conn.bytes_read]) {
                conn.body_start_location = start;
                return self.handle_request(conn);
            }
            Ok(ConnectionState::ReadingRequest)
        }
    }

    fn handle_post(
        &self,
        req: &HttpRequest,
        conn: &mut HttpConnection,
    ) -> Result<HttpResult, io::Error> {
        let boundary = match get_post_boundary(req) {
            Some(b) => b,
            None => {
                return Ok(HttpResult::Error(
                    HttpStatus::BadRequest,
                    Some(format!(
                        "Failed to find or parse boundary: {}",
                        match req.get_header("content-type") {
                            Some(ct) => ct,
                            None => "[ Missing ]",
                        }
                    )),
                ));
            }
        };

        let real_boundary = format!("--{}", boundary);
        let post_delimeter = match BMByte::from(real_boundary.clone()) {
            Some(bmb) => bmb,
            None => {
                return Ok(HttpResult::Error(
                    HttpStatus::ServerError,
                    Some(format!(
                        "Could not create Boyer-Moore delimeter for the given boundary: {}",
                        boundary
                    )),
                ));
            }
        };

        let normalized_path = if req.path.starts_with("/") {
            &req.path[1..]
        } else {
            &req.path[..]
        };

        let path = self.root_dir.join(normalized_path);

        let canonical_path = match get_and_check_canon_path(&self.root_dir, path)? {
            Some(path) => path,
            None => {
                return Ok(HttpResult::Error(
                    HttpStatus::NotFound,
                    Some("Path disallowed.".to_string()),
                ));
            }
        };

        let mut pb = PostBuffer::new(canonical_path, post_delimeter, real_boundary);
        pb.fill_location = conn.bytes_read - conn.body_start_location;
        &pb.buffer[..pb.fill_location]
            .clone_from_slice(&conn.buffer[conn.body_start_location..conn.bytes_read]);

        conn.post_buffer = Some(pb);
        Ok(HttpResult::ReadRequestBody)
    }

    fn handle_get(&self, req: &HttpRequest) -> Result<HttpResult, io::Error> {
        let normalized_path = if req.path.starts_with("/") {
            &req.path[1..]
        } else {
            &req.path[..]
        };

        let path = self.root_dir.join(normalized_path);
        let canonical_path = match get_and_check_canon_path(&self.root_dir, path)? {
            Some(path) => path,
            None => {
                return Ok(HttpResult::Error(
                    HttpStatus::NotFound,
                    Some("Path disallowed.".to_string()),
                ));
            }
        };

        let metadata = match fs::metadata(&canonical_path) {
            Err(error) => {
                return match resolve_io_error(&error) {
                    Some(http_error) => Ok(HttpResult::Error(http_error, Some(error.to_string()))),
                    None => Err(error),
                };
            }
            Ok(data) => data,
        };

        if !metadata.is_file() && !metadata.is_dir() {
            return Ok(HttpResult::Error(
                HttpStatus::PermissionDenied,
                Some(format!("Attempted to read an irregular file.")),
            ));
        }

        if !self.dir_listings && metadata.is_dir() {
            return Ok(HttpResult::Error(
                HttpStatus::PermissionDenied,
                Some(format!("Unable to list this directory.")),
            ));
        }

        let (mut response_data, full_length, mime) = if metadata.is_dir() {
            let s: String = rendering::render_directory(normalized_path, canonical_path.as_path());
            let len = s.len();
            let data = ResponseDataType::String(SeekableString::new(s));
            (data, len, Some("text/html"))
        } else {
            let data = ResponseDataType::File(fs::File::open(&canonical_path)?);
            let len = if metadata.is_file() {
                metadata.len() as usize
            } else {
                std::u32::MAX as usize
            };
            // (data, len, None)
            (
                data,
                len,
                if req.path.ends_with(".html") {
                    Some("text/html")
                } else {
                    None
                },
            )
        };

        let (start, range, used_range) = match req.get_header("range") {
            Some(content_range_str) => {
                if let Some(content_range) = decode_content_range(content_range_str) {
                    let real_start = min(content_range.start, full_length);
                    let real_len = match content_range.len {
                        Some(len) => min(len, full_length - real_start),
                        None => full_length - real_start,
                    };
                    (real_start, real_len, true)
                } else {
                    return Ok(HttpResult::Error(
                        HttpStatus::BadRequest,
                        Some(format!("Could not decode Range header")),
                    ));
                }
            }
            None => (0, full_length, false),
        };

        let mut resp = HttpResponse::new(
            if used_range {
                HttpStatus::PartialContent
            } else {
                HttpStatus::OK
            },
            &req.version,
        );

        resp.add_header("Server".to_string(), "http-tui".to_string());
        resp.add_header("Accept-Ranges".to_string(), "bytes".to_string());

        resp.set_content_length(range);

        if used_range {
            resp.add_header(
                "Content-Range".to_string(),
                format!(
                    "bytes {}-{}/{}",
                    start,
                    max(start, start + range - 1),
                    full_length
                ),
            );
            match response_data {
                ResponseDataType::String(ref mut seg) => {
                    seg.seek(io::SeekFrom::Start((start) as u64))?;
                }
                ResponseDataType::File(ref mut file) => {
                    file.seek(io::SeekFrom::Start((start) as u64))?;
                }
                _ => {}
            }
        }

        if let Some(content_type) = mime {
            // If we want to add a content type, add it
            resp.add_header("Content-Type".to_string(), content_type.to_string());
        }

        resp.add_body(response_data);

        Ok(HttpResult::Response(resp, range))
    }

    fn parse_and_service_request(
        &self,
        mut conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        let head = &mut conn.buffer[..conn.body_start_location];

        let req: HttpRequest = match decode_request(head) {
            Ok(r) => r,
            Err(status) => {
                // Kill the connection if we get invalid data
                conn.keep_alive = false;
                return self.create_oneoff_response(
                    status,
                    conn,
                    Some("Could not decode request.".to_string()),
                );
            }
        };

        conn.last_requested_uri = Some(req.path.to_string());
        conn.last_requested_method = req.method.clone();
        conn.num_requests += 1;

        if self.disabled {
            conn.keep_alive = false;
            return self.create_oneoff_response(HttpStatus::ServiceUnavailable,
                                              conn, Some("This server has been temporarily disabled. Please contact the administrator to re-enable it.".to_string()));
        }

        // Check if keep-alive header was given in the request.
        // If it was not, assume keep-alive is >= HTTP/1.1.
        conn.keep_alive = match req.get_header("connection") {
            Some(value) => value.to_lowercase() == "keep-alive",
            None => false,
        };

        let maybe_result = match req.method {
            None => {
                return self.create_oneoff_response(
                    HttpStatus::NotImplemented,
                    conn,
                    Some("This server does not implement the requested HTTP method.".to_string()),
                );
            }
            Some(HttpMethod::GET) => self.handle_get(&req),
            Some(HttpMethod::HEAD) => self.handle_get(&req),
            Some(HttpMethod::POST) => self.handle_post(&req, conn),
        };
        let result = match maybe_result {
            // Attempt to convert the system error into an HTTP error
            // that we can send back to the user.
            Ok(r) => r,
            Err(error) => match resolve_io_error(&error) {
                Some(http_error) => HttpResult::Error(http_error, Some(error.to_string())),
                None => {
                    return Err(error);
                }
            },
        };

        let (mut resp, range) = match result {
            HttpResult::Error(http_status, msg) => {
                return self.create_oneoff_response(http_status, conn, msg);
            }
            HttpResult::ReadRequestBody => {
                return self.check_partial_post_body(conn);
            }
            HttpResult::Response(resp, range) => (resp, range),
        };

        resp.add_header(
            "Connection".to_string(),
            if conn.keep_alive {
                "keep-alive".to_string()
            } else {
                "close".to_string()
            },
        );

        // Write headers
        resp.write_headers_to_stream(&conn.stream)?;

        // If method is HEAD, remove the response body
        if req.method.unwrap_or(HttpMethod::HEAD) == HttpMethod::HEAD {
            resp.clear_body();
        }

        conn.response = Some(resp);
        conn.bytes_requested += range;

        Ok(ConnectionState::WritingResponse)
    }

    fn write_partial_response(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        let done = match &mut conn.response {
            Some(ref mut resp) => {
                let amt_written = resp.partial_write_to_stream(&conn.stream)?;
                conn.bytes_sent += amt_written;
                // If we wrote nothing, we are done
                amt_written == 0 || conn.bytes_sent >= conn.bytes_requested
            }
            None => true,
        };

        if done {
            if conn.keep_alive {
                // Reset the data associated with this connection
                conn.reset();
                return Ok(ConnectionState::ReadingRequest);
            } else {
                return Ok(ConnectionState::Closing);
            }
        }

        Ok(ConnectionState::WritingResponse)
    }

    fn create_http_connection(stream: TcpStream) -> HttpConnection {
        HttpConnection::new(stream)
    }

    fn handle_conn_sigpipe(&self, conn: &mut HttpConnection) -> Result<(), io::Error> {
        match self.handle_conn(conn) {
            Err(error) => {
                conn.state = ConnectionState::Closing;
                match error.kind() {
                    io::ErrorKind::BrokenPipe => Ok(()),
                    io::ErrorKind::ConnectionReset => Ok(()),
                    io::ErrorKind::ConnectionAborted => Ok(()),
                    // Forward the error if it isn't one of the above
                    _ => Err(error),
                }
            }
            _ => Ok(()),
        }
    }

    fn check_partial_post_body(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        let pb = &mut conn.post_buffer.as_mut().unwrap();
        match pb.handle_new_data() {
            Ok(done) => {
                if done {
                    return self.create_oneoff_response(
                        HttpStatus::OK,
                        conn,
                        Some(format!("File successfully uploaded")),
                    );
                } else {
                    Ok(ConnectionState::ReadingPostBody)
                }
            }
            Err(s) => {
                return self.create_oneoff_response(
                    HttpStatus::ServerError,
                    conn,
                    Some(format!("Error while parsing POST request: {}", s)),
                );
            }
        }
    }

    fn read_partial_post_body(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        if let Some(pb) = &mut conn.post_buffer {
            let bytes_read = match conn.stream.read(&mut pb.buffer[pb.fill_location..]) {
                Ok(size) => size,
                Err(_err) => {
                    // Even though the server has run into a problem, because it is
                    // a problem inherent to the socket connection, we return Ok
                    // so that we do not write an HTTP error response to the socket.
                    return Ok(ConnectionState::Closing);
                }
            };
            conn.bytes_read += bytes_read;
            pb.fill_location += bytes_read;

            if bytes_read == 0 {
                let res = self.create_oneoff_response(
                    HttpStatus::BadRequest,
                    conn,
                    Some("An error occurred while receiving your file.".to_string()),
                );
                let _ = self.write_conn_to_history(conn);
                return res;
            }

            self.check_partial_post_body(conn)
        } else {
            return self.create_oneoff_response(
                HttpStatus::ServerError,
                conn,
                Some("Attempt to read POST contents without a buffer.".to_string()),
            );
        }
    }

    fn handle_conn(&self, conn: &mut HttpConnection) -> Result<(), io::Error> {
        match conn.state {
            ConnectionState::ReadingRequest => {
                conn.state = self.read_partial_request(conn)?;
            }
            ConnectionState::ReadingPostBody => {
                conn.state = self.read_partial_post_body(conn)?;
            }
            ConnectionState::WritingResponse => {
                conn.state = self.write_partial_response(conn)?;
            }
            ConnectionState::Closing => {}
        }

        Ok(())
    }

    fn create_oneoff_response(
        &self,
        status: HttpStatus,
        mut conn: &mut HttpConnection,
        msg: Option<String>,
    ) -> Result<ConnectionState, io::Error> {
        let body: String = rendering::render_error(&status, msg);
        let mut resp = HttpResponse::new(status, &HttpVersion::Http1_0);
        resp.add_header("Server".to_string(), "http-tui".to_string());
        resp.set_content_length(body.len());
        resp.add_header(
            "Connection".to_string(),
            if conn.keep_alive {
                "keep-alive".to_string()
            } else {
                "close".to_string()
            },
        );
        resp.add_header("Content-Type".to_string(), "text/html".to_string());

        // Add content-length to bytes requested
        conn.bytes_requested += body.len();

        let data = ResponseDataType::String(SeekableString::new(body));

        // Write headers
        resp.write_headers_to_stream(&conn.stream)?;
        resp.add_body(data);

        conn.response = Some(resp);

        Ok(ConnectionState::WritingResponse)
    }
}

fn find_body_start(buffer: &[u8]) -> Option<usize> {
    lazy_static! {
        static ref BODY_DELIM: BMByte = BMByte::from("\r\n\r\n").unwrap();
    };

    let vec = BODY_DELIM.find_in(BMBuf(buffer), 1);
    if vec.len() < 1 {
        None
    } else {
        Some(vec[0] + 4)
    }
}

fn get_post_boundary(req: &HttpRequest) -> Option<&str> {
    let ct = req.get_header("content-type")?;
    for segment in ct.split(";") {
        if segment.trim_start().starts_with("boundary=") {
            let (_, inner) = segment.split_at(segment.find("=")?);

            // Remove the surrounding quotes
            if inner.starts_with("=\"") {
                return Some(&inner[2..inner.len() - 1]);
            }

            return Some(&inner[1..]);
        }
    }
    None
}

fn get_and_check_canon_path(root_dir: &Path, path: PathBuf) -> Result<Option<PathBuf>, io::Error> {
    let canonical_path = match fs::canonicalize(path) {
        Err(error) => {
            return Err(error);
        }
        Ok(path) => path,
    };

    if !canonical_path.starts_with(root_dir) {
        // Use 404 so that the user cannot determine if directories
        // exist or not.
        return Ok(None);
    }

    Ok(Some(canonical_path))
}
