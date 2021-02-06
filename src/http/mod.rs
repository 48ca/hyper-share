mod boyer_moore;
mod http_core;
mod post_buffer;
mod rendering;

use post_buffer::PostBuffer;

use nix::unistd;

use std::path::PathBuf;

use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;

use std::cmp::{max, min};

use std::str::from_utf8;

use std::io::{self, Read, Seek};

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

use boyer_moore_magiclen::BMByte;

const BUFFER_SIZE: usize = 4096;

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

#[derive(PartialEq, Debug)]
pub enum ConnectionState {
    ReadingRequest,
    ReadingPostBody,
    WritingResponse,
    Closing,
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
        self.post_buffer = None;
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
    uploading: bool,
}

impl HttpTui<'_> {
    pub fn new<'a>(
        host: &str,
        port: u16,
        root_dir: &'a Path,
        sender: mpsc::Sender<String>,
        dir_listings: bool,
        disabled: bool,
        uploading: bool,
    ) -> Result<HttpTui<'a>, io::Error> {
        let listener = TcpListener::bind(format!("{mask}:{port}", mask = host, port = port))?;
        Ok(HttpTui {
            listener: listener,
            root_dir: root_dir,
            history_channel: sender,
            dir_listings: dir_listings,
            disabled: disabled,
            uploading: uploading,
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

            let mut force_close: bool = false;

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
                                if buf[0] as char == 'k' {
                                    force_close = true;
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
                .filter(|&(_, conn)| conn.state == ConnectionState::Closing || force_close)
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
        if bytes_read == 0 {
            return Ok(ConnectionState::Closing);
        } else if conn.bytes_read == buffer.len() {
            if let Some(start) = boyer_moore::find_body_start(&conn.buffer[..conn.bytes_read]) {
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
            if let Some(start) = boyer_moore::find_body_start(&conn.buffer[..conn.bytes_read]) {
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
        if !self.uploading {
            return Ok(HttpResult::Error(
                HttpStatus::MethodNotAllowed,
                Some(format!("This server does not accept POST requests.")),
            ));
        }

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

        let pb = PostBuffer::new(
            canonical_path,
            post_delimeter,
            real_boundary,
            &conn.buffer[conn.body_start_location..conn.bytes_read],
        );

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
            let s: String = rendering::render_directory(
                normalized_path,
                canonical_path.as_path(),
                self.uploading,
            );
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

        resp.add_header("Server".to_string(), "hypershare".to_string());
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
        match pb.handle_new_data_queue_error() {
            Ok(done) => {
                if done {
                    self.create_oneoff_response(
                        HttpStatus::Created,
                        conn,
                        Some(format!("File received.")),
                    )
                } else {
                    Ok(ConnectionState::ReadingPostBody)
                }
            }
            Err(s) => {
                conn.keep_alive = false;
                self.create_oneoff_response(
                    HttpStatus::ServerError,
                    conn,
                    Some(format!("Error while parsing POST request: {}", s)),
                )
            }
        }
    }

    fn read_partial_post_body(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        if let Some(pb) = &mut conn.post_buffer {
            let bytes_read = match conn.stream.read(pb.get_open_slice()) {
                Ok(size) => size,
                Err(_err) => {
                    // Even though the server has run into a problem, because it is
                    // a problem inherent to the socket connection, we return Ok
                    // so that we do not write an HTTP error response to the socket.
                    return Ok(ConnectionState::Closing);
                }
            };
            conn.bytes_read += bytes_read;
            pb.update_fill_location(bytes_read);

            if bytes_read == 0 {
                let res = self.create_oneoff_response(
                    HttpStatus::BadRequest,
                    conn,
                    Some("An error occurred while receiving your file.".to_string()),
                );
                let _ = self.write_conn_to_history(conn);
                return res;
            }

            let res = self.check_partial_post_body(conn);
            match res {
                Ok(ConnectionState::ReadingPostBody) => {}
                _ => {
                    let _ = self.write_conn_to_history(conn);
                }
            };

            res
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
        let mut resp = HttpResponse::new(status.clone(), &HttpVersion::Http1_1);
        resp.add_header("Server".to_string(), "hypershare".to_string());

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

        assert_eq!(conn.response.is_none(), true);
        conn.response = Some(resp);

        Ok(ConnectionState::WritingResponse)
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
