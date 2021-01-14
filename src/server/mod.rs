mod http;
mod rendering;

use std::net::SocketAddr;
use std::net::TcpListener;
use std::net::TcpStream;

use std::cmp::{max, min};

use std::io;
use std::io::{Read, Seek, SeekFrom};

use std::str::from_utf8;

use std::fs;

use std::format;

use std::collections::HashMap;

use nix::sys::select::{select, FdSet};
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;

use std::sync::mpsc;

use std::path::Path;

use http::{HttpMethod, HttpRequest, HttpResponse, HttpStatus, HttpVersion};

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

fn end_of_http_request(req_body: &[u8]) -> bool {
    if req_body.len() < 4 {
        return false;
    }
    return &req_body[req_body.len() - 4..] == b"\r\n\r\n";
}

#[derive(PartialEq, Debug)]
pub enum ConnectionState {
    ReadingRequest,
    WritingResponse,
    Closing,
}

pub struct SeekableString {
    pub start: usize,
    pub data: String,
}

impl SeekableString {
    pub fn new(d: String) -> SeekableString {
        SeekableString { start: 0, data: d }
    }
}

impl Read for SeekableString {
    fn read(&mut self, buf: &mut [u8]) -> Result<usize, io::Error> {
        let mut slice = &self.data.as_bytes()[self.start..];
        let read = slice.read(buf)?;
        self.start += read;
        Ok(read)
    }
}

impl Seek for SeekableString {
    fn seek(&mut self, pos: SeekFrom) -> Result<u64, io::Error> {
        self.start = match pos {
            SeekFrom::Start(i) => i as usize,
            SeekFrom::Current(i) => ((self.start as i64) + i) as usize,
            SeekFrom::End(i) => ((self.data.len() as i64) - i) as usize,
        };
        Ok(self.start as u64)
    }
}

pub enum ResponseDataType {
    None,
    StringSegment(SeekableString),
    FileSegment(fs::File),
}

pub struct HttpConnection {
    pub stream: TcpStream,
    pub state: ConnectionState,

    // Buffer for holding a pending request
    pub buffer: [u8; BUFFER_SIZE],
    pub bytes_read: usize,

    // Space to store a per-request string response
    pub response_data: ResponseDataType,
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
            buffer: [0; BUFFER_SIZE],
            bytes_read: 0,
            response_data: ResponseDataType::None,
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
    Response(HttpResponse, ResponseDataType, usize),
    Error(HttpStatus, Option<String>),
}

pub struct HttpTui<'a> {
    listener: TcpListener,
    root_dir: &'a Path,
    history_channel: mpsc::Sender<String>,
}

impl HttpTui<'_> {
    pub fn new<'a>(
        host: &str,
        port: u16,
        root_dir: &'a Path,
        sender: mpsc::Sender<String>,
    ) -> Result<HttpTui<'a>, io::Error> {
        let listener = TcpListener::bind(format!("{mask}:{port}", mask = host, port = port))?;
        Ok(HttpTui {
            listener: listener,
            root_dir: root_dir,
            history_channel: sender,
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

                        // If we have data to read on the pipe, we need to close
                        if fd == pipe_read {
                            break 'main;
                        }
                        let peer_fd = if fd == l_raw_fd {
                            // If listener, get accept new connection and add it.
                            // Then pass back the new peer fd to do an immediate read
                            match self.listener.accept() {
                                Ok((stream, _addr)) => {
                                    let conn = HttpTui::create_http_connection(stream);
                                    let pfd = conn.stream.as_raw_fd();
                                    connections.insert(pfd, conn);
                                    pfd
                                }
                                _ => {
                                    continue;
                                }
                            }
                        } else {
                            // We have selected a peer fd, so just use that one
                            fd
                        };
                        assert_eq!(connections[&peer_fd].state, ConnectionState::ReadingRequest);
                        // TODO: Error checking here
                        let mut conn = connections.get_mut(&peer_fd).unwrap();
                        match self.handle_conn_sigpipe(&mut conn) {
                            Ok(_) => {}
                            Err(error) => {
                                let _ = self.history_channel.send(format!("Uncaught OS error while handling connection: {}", error));
                                // write_error(format!("Server error while reading: {}", error));
                            }
                        };
                        if connections[&peer_fd].state == ConnectionState::Closing {
                            // Delete to close connection
                            connections.remove(&peer_fd);
                        }
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
                        if connections[&fd].state == ConnectionState::Closing {
                            // Delete to close connection
                            connections.remove(&fd);
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
            func(&connections);
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
        if bytes_read == 0 || end_of_http_request(&buffer[..conn.bytes_read]) {
            if conn.bytes_read == 0 {
                return Ok(ConnectionState::Closing);
            }
            // Once we have read the request, handle it.
            // The connection state will be updated accordingly
            let res = self.handle_request(conn);
            if let Ok(peer_addr) = conn.stream.peer_addr() {
                let ip_str = match peer_addr {
                    SocketAddr::V4(addr) => format!("{}:{}", addr.ip(), addr.port()),
                    SocketAddr::V6(addr) => format!("[{}]:{}", addr.ip(), addr.port()),
                };
                let code_str = match &conn.response {
                    Some(resp) => resp.get_code(),
                    None => "???".to_string(),
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

            let state = match res {
                Ok(state) => { state },
                Err(error) => {
                    match self.write_error_response(
                        HttpStatus::ServerError,
                        conn,
                        Some(error.to_string()),
                    ) {
                        Ok(state) => { state },
                        Err(e) => { return Err(e) },
                    }
                }
            };

            if state == ConnectionState::WritingResponse {
                // Force an initial write of the data
                self.write_partial_response(conn)
            } else {
                Ok(state)
            }
        } else {
            Ok(ConnectionState::ReadingRequest)
        }
    }

    fn handle_post(&self, req: &HttpRequest) -> Result<HttpResult, io::Error> {
        let resp = HttpResponse::new(HttpStatus::OK, &req.version);
        Ok(HttpResult::Response(resp, ResponseDataType::None, 0))
    }

    fn handle_get(&self, req: &HttpRequest) -> Result<HttpResult, io::Error> {
        let normalized_path = if req.path.starts_with("/") {
            &req.path[1..]
        } else {
            &req.path[..]
        };

        let path = self.root_dir.join(normalized_path);
        let canonical_path = match fs::canonicalize(path) {
            Err(error) => {
                // Attempt to convert the system error into an HTTP error
                // that we can send back to the user.
                return match resolve_io_error(&error) {
                    Some(http_error) => Ok(HttpResult::Error(http_error, Some(error.to_string()))),
                    None => Err(error),
                };
            }
            Ok(path) => path,
        };

        if !canonical_path.starts_with(self.root_dir) {
            // Use 404 so that the user cannot determine if directories
            // exist or not.
            return Ok(HttpResult::Error(
                HttpStatus::NotFound,
                Some("Path disallowed.".to_string()),
            ));
        }

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

        let (mut response_data, full_length, mime) = if metadata.is_dir() {
            let s: String = rendering::render_directory(normalized_path, canonical_path.as_path());
            let len = s.len();
            let data = ResponseDataType::StringSegment(SeekableString::new(s));
            (data, len, Some("text/html"))
        } else {
            let data = ResponseDataType::FileSegment(fs::File::open(&canonical_path)?);
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
                ResponseDataType::StringSegment(ref mut seg) => {
                    seg.seek(io::SeekFrom::Start((start) as u64))?;
                }
                ResponseDataType::FileSegment(ref mut file) => {
                    file.seek(io::SeekFrom::Start((start) as u64))?;
                }
                _ => {}
            }
        }

        if let Some(content_type) = mime {
            // If we want to add a content type, add it
            resp.add_header("Content-Type".to_string(), content_type.to_string());
        }

        Ok(HttpResult::Response(resp, response_data, range))
    }

    fn handle_request(&self, mut conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
        let body = &mut conn.buffer[..conn.bytes_read];

        let req: HttpRequest = match decode_request(body) {
            Ok(r) => r,
            Err(status) => {
                // Kill the connection if we get invalid data
                conn.keep_alive = false;
                return self.write_error_response(
                    status,
                    conn,
                    Some("Could not decode request.".to_string()),
                );
            }
        };

        conn.last_requested_uri = Some(req.path.to_string());
        conn.last_requested_method = req.method.clone();
        conn.num_requests += 1;

        // Check if keep-alive header was given in the request.
        // If it was not, assume keep-alive is >= HTTP/1.1.
        conn.keep_alive = match req.get_header("connection") {
            Some(value) => value.to_lowercase() == "keep-alive",
            None => false,
        };

        let result = match req.method {
            None => {
                return self.write_error_response(
                    HttpStatus::NotImplemented,
                    conn,
                    Some("This server does not implement the requested HTTP method.".to_string()),
                );
            }
            Some(HttpMethod::GET) => self.handle_get(&req)?,
            Some(HttpMethod::HEAD) => self.handle_get(&req)?,
            Some(HttpMethod::POST) => self.handle_post(&req)?,
        };

        let (mut resp, response_data, range) = match result {
            HttpResult::Error(http_status, msg) => {
                return self.write_error_response(http_status, conn, msg);
            }
            HttpResult::Response(resp, response_data, range) => (resp, response_data, range),
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

        conn.response = Some(resp);

        conn.response_data = match req.method.unwrap() {
            HttpMethod::HEAD => ResponseDataType::None,
            _ => {
                conn.bytes_requested += range;
                response_data
            }
        };

        Ok(ConnectionState::WritingResponse)
    }

    fn write_partial_response(
        &self,
        conn: &mut HttpConnection,
    ) -> Result<ConnectionState, io::Error> {
        let done = match &mut conn.response {
            Some(ref mut resp) => {
                let amt_written = match &mut conn.response_data {
                    ResponseDataType::StringSegment(ref mut seg) => {
                        resp.partial_write_to_stream(seg, &conn.stream)?
                    }
                    ResponseDataType::FileSegment(ref mut file) => {
                        resp.partial_write_to_stream(file, &conn.stream)?
                    }
                    ResponseDataType::None => 0,
                };
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

        return Ok(ConnectionState::WritingResponse);
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

    fn handle_conn(&self, conn: &mut HttpConnection) -> Result<(), io::Error> {
        match conn.state {
            ConnectionState::ReadingRequest => {
                conn.state = self.read_partial_request(conn)?;
            }
            ConnectionState::WritingResponse => {
                conn.state = self.write_partial_response(conn)?;
            }
            ConnectionState::Closing => {}
        }

        Ok(())
    }

    fn write_error_response(
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

        let data = ResponseDataType::StringSegment(SeekableString::new(body));

        // Write headers
        resp.write_headers_to_stream(&conn.stream)?;

        conn.response = Some(resp);
        conn.response_data = data;

        Ok(ConnectionState::WritingResponse)
    }
}
