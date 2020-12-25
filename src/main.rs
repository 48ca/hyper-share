use std::net::TcpListener;
use std::net::TcpStream;
use std::net::SocketAddr::{V4,V6};

use std::io;
use std::io::Read;

use std::str::from_utf8;

use std::fs;

use std::format;

use std::collections::HashMap;

use nix::sys::select::{select,FdSet};
use std::os::unix::io::AsRawFd;
use std::os::unix::prelude::RawFd;

mod simple_http;
use simple_http::{
    HttpRequest, HttpResponse, HttpStatus, HttpVersion,
    status_to_code, status_to_message
};

const BUFFER_SIZE: usize = 4096;

fn write_error(error_str: String) {
    eprintln!("An error occurred: {}", error_str);
}

fn resolve_io_error(error: &io::Error) -> Option<HttpStatus> {
    match error.kind() {
        io::ErrorKind::NotFound => Some(HttpStatus::NotFound),
        io::ErrorKind::PermissionDenied => Some(HttpStatus::PermissionDenied),
        _ => None
    }
}

fn decode_request(req_body: &[u8]) -> Result<HttpRequest, HttpStatus> {
    let request_str = match from_utf8(req_body) {
        Ok(dec) => dec,
        Err(err) => {
            write_error(format!("Could not decode request: {}", err));
            return Err(HttpStatus::BadRequest);
        }
    };

    return HttpRequest::new(request_str);
}

fn end_of_http_request(req_body: &[u8]) -> bool {
    if req_body.len() < 4 { return false; }
    return &req_body[req_body.len() - 4 ..] == b"\r\n\r\n";
}

#[derive(PartialEq, Debug)]
enum ConnectionState {
    ReadingRequest,
    WritingResponse,
    Closing
}

fn write_error_response(status: HttpStatus, mut conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
    let body = format!("<html><body><h1>{} {}</h1></body></html>",
                       status_to_code(&status),
                       status_to_message(&status));
    let mut resp = HttpResponse::new(status, HttpVersion::Http1_0);
    resp.set_content_length(body.len());
    resp.add_header("Connection".to_string(),
                    if conn.keep_alive { "keep-alive".to_string() }
                    else { "close".to_string() });

    let data = ResponseDataType::StringData(StringSegment {
        start: 0,
        data: body,
    });

    // Write headers
    resp.write_headers_to_stream(&conn.stream)?;

    conn.response = Some(resp);
    conn.response_data = data;

    // Force an initial write of the data
    write_partial_response(&mut conn)
}

struct StringSegment {
    pub start: usize,
    pub data: String,
}
struct FileSegment {
    pub file: fs::File,
}

enum ResponseDataType {
    None,
    StringData(StringSegment),
    FileData(FileSegment),
}

struct HttpConnection {
    pub stream: TcpStream,
    pub state: ConnectionState,

    // Buffer for holding a pending request
    pub buffer: [u8; BUFFER_SIZE],
    pub bytes_read: usize,

    // Space to store a per-request string response
    pub response_data: ResponseDataType,
    pub response: Option<HttpResponse>,

    pub keep_alive: bool,
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
        }
    }

    pub fn reset(&mut self) {
        self.bytes_read = 0;
        self.response = None;
    }
}

fn read_partial_request(conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
    let buffer = &mut conn.buffer;
    let bytes_read = match conn.stream.read(&mut buffer[conn.bytes_read..]) {
        Ok(size) => size,
        Err(err) => {
            write_error(format!(
                "Failed to read bytes from socket: {}", err));
            // Even though the server has run into a problem, because it is
            // a problem inherent to the socket connection, we return Ok
            // so that we do not write an HTTP error response to the socket.
            return Ok(ConnectionState::Closing);
        }
    };

    conn.bytes_read += bytes_read;
    if bytes_read == 0 || end_of_http_request(&buffer[..conn.bytes_read]) {
        // Once we have read the request, handle it.
        // The connection state will be updated accordingly
        handle_request(conn)
    } else {
        Ok(ConnectionState::ReadingRequest)
    }
}

fn handle_request(mut conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
    let body = &mut conn.buffer[..conn.bytes_read];

    let req: HttpRequest = match decode_request(body) {
        Ok(r) => r,
        Err(status) => {
            return write_error_response(status, conn);
        }
    };

    // Check if keep-alive header was given in the request.
    // If it was not, assume keep-alive is >= HTTP/1.1.
    conn.keep_alive = match req.get_header("connection") {
        Some(value) => value.to_lowercase() == "keep-alive",
        None => req.version != HttpVersion::Http1_0
    };

    if req.method.is_none() {
        return write_error_response(HttpStatus::NotImplemented, conn);
    }

    let canonical_path = match fs::canonicalize(req.path) {
        Err(error) => {
            // Attempt to convert the system error into an HTTP error
            // that we can send back to the user.
            return match resolve_io_error(&error) {
                Some(http_error) => write_error_response(http_error, conn),
                None => Err(error),
            };
        }
        Ok(path) => path
    };

    let metadata = match fs::metadata(&canonical_path) {
        Err(error) => {
            return match resolve_io_error(&error) {
                Some(http_error) => write_error_response(http_error, conn),
                None => Err(error),
            };
        }
        Ok(data) => data
    };

    if !metadata.is_file() && !metadata.is_dir() {
        return write_error_response(HttpStatus::PermissionDenied, conn);
    }

    // Fix hard-coding DEFAULT_HTTP_VERSION here
    let mut resp = HttpResponse::new(HttpStatus::OK, req.version);

    let (response_data, content_length) = if metadata.is_file() {
            let data = ResponseDataType::FileData(FileSegment {
                file: fs::File::open(&canonical_path)?
            });
            let len = metadata.len() as usize;
            (data, len)
        } else {
            let s: &'static str = "<html><body>Directory listing isn't implemented yet!</body></html>";
            let data = ResponseDataType::StringData(StringSegment {
                start: 0,
                data: s.to_string(),
            });
            (data, s.len())
        };

    resp.set_content_length(content_length);
    resp.add_header("Connection".to_string(),
                    if conn.keep_alive { "keep-alive".to_string() }
                    else { "close".to_string() });

    // Write headers
    resp.write_headers_to_stream(&conn.stream)?;

    conn.response = Some(resp);

    conn.response_data = response_data;

    // Force an initial write of the data
    write_partial_response(&mut conn)
}

fn write_partial_response(conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
    let done = match &mut conn.response {
        Some(ref mut resp) => {
            match &mut conn.response_data {
                ResponseDataType::StringData(seg) => {
                    let bytes = &mut seg.data.as_bytes();
                    // TODO: Please fix hard-coding 4096 here
                    let res = !resp.partial_write_to_stream(bytes, &conn.stream)?;
                    if res { seg.start += 4096; }
                    res
                }
                ResponseDataType::FileData(seg) => {
                    resp.partial_write_to_stream(&mut seg.file, &conn.stream)?
                }
                ResponseDataType::None => true
            }
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
    // Print that the connection has been established
    let peer_addr = stream.peer_addr().unwrap();
    match peer_addr {
        V4(v4_addr) => {
            println!("Connection established: {host}:{port}",
                host=v4_addr.ip(), port=v4_addr.port());
        }
        V6(v6_addr) => {
            println!("Connection established: [{host}]:{port}",
            host=v6_addr.ip(), port=v6_addr.port());
        }
    }

    HttpConnection::new(stream)
}

fn handle_conn_sigpipe(conn: &mut HttpConnection) -> Result<(), io::Error> {
    match handle_conn(conn) {
        Err(error) => {
            match error.kind() {
                io::ErrorKind::BrokenPipe => {
                    conn.state = ConnectionState::Closing;
                    Ok(())
                },
                // Forward the error if it wasn't broken pipe
                _ => Err(error)
            }
        },
        _ => Ok(())
    }
}

fn handle_conn(conn: &mut HttpConnection) -> Result<(), io::Error> {
    match conn.state {
        ConnectionState::ReadingRequest => {
            conn.state = read_partial_request(conn)?;
        }
        ConnectionState::WritingResponse => {
            conn.state = write_partial_response(conn)?;
        }
        ConnectionState::Closing => {}
    }

    Ok(())
}

fn main() {
    let port: u16 = 8080;
    let mask: &'static str = "0.0.0.0";
    let listener = TcpListener::bind(format!("{mask}:{port}", mask=mask, port=port)).unwrap();

    let mut connections = HashMap::<RawFd, HttpConnection>::new();

    let l_raw_fd = listener.as_raw_fd();

    loop {
        let mut r_fds = FdSet::new();
        let mut w_fds = FdSet::new();
        let mut e_fds = FdSet::new();

        // First add listener:
        r_fds.insert(l_raw_fd);
        e_fds.insert(l_raw_fd);

        for (fd, http_conn) in &connections {
            match http_conn.state {
                ConnectionState::WritingResponse => { w_fds.insert(*fd); }
                ConnectionState::ReadingRequest  => { r_fds.insert(*fd); }
                _ => {}
            }
            e_fds.insert(*fd);
        }

        match select(None, Some(&mut r_fds), Some(&mut w_fds), Some(&mut e_fds), None) {
            Ok(_res) => {},
            Err(e) => {
                println!("Got error while selecting: {}", e);
                break;
            }
        }

        match r_fds.highest() {
            None => {},
            Some(mfd) => {
                for fd in 0..(mfd + 1) {
                    if !r_fds.contains(fd) { continue; }
                    // if !connections.contains_key(&fd) { continue; }
                    // If listener, get accept new connection and add it.
                    if fd == l_raw_fd {
                        match listener.accept() {
                            Ok((stream, _addr)) => {
                                let conn = create_http_connection(stream);
                                connections.insert(conn.stream.as_raw_fd(), conn);
                            }
                            Err(e) => write_error(e.to_string()),
                        }
                    } else {
                        assert_eq!(connections[&fd].state, ConnectionState::ReadingRequest);
                        // TODO: Error checking here
                        let mut conn = connections.get_mut(&fd).unwrap();
                        match handle_conn_sigpipe(&mut conn) {
                            Ok(_) => {},
                            Err(error) => {
                                let _ = write_error_response(HttpStatus::ServerError, conn);
                                write_error(format!("Server error while reading: {}", error));
                            }
                        };
                        if connections[&fd].state == ConnectionState::Closing {
                            // Delete to close connection
                            connections.remove(&fd);
                        }
                    }
                }
            }
        }
        match w_fds.highest() {
            None => {},
            Some(mfd) => {
                for fd in 0..(mfd + 1) {
                    if !w_fds.contains(fd) { continue; }
                    // if !connections.contains_key(&fd) { continue; }
                    assert_eq!(connections[&fd].state, ConnectionState::WritingResponse);
                    match handle_conn_sigpipe(&mut connections.get_mut(&fd).unwrap()) {
                        Ok(_) => {},
                        Err(error) => {
                            write_error(format!("Server error while writing: {}", error));
                        }
                    }
                    if connections[&fd].state == ConnectionState::Closing {
                        // Delete to close connection
                        connections.remove(&fd);
                    }
                }
            }
        }
        match e_fds.highest() {
            None => {},
            Some(mfd) => {
                for fd in 0..(mfd + 1) {
                    if !e_fds.contains(fd) { continue; }
                    // if !connections.contains_key(&fd) { continue; }
                    // If listener, get accept new connection and add it.
                    if fd == l_raw_fd {
                        eprintln!("Listener socket has errored!");
                        break;
                    } else {
                        // Ignore the return value of write_error_response, because
                        // we're closing the connection anyway.
                        let _ = write_error_response(HttpStatus::ServerError, connections.get_mut(&fd).unwrap());
                        println!("Got bad state on client socket");
                        connections.remove(&fd);
                    }
                }
            }
        }
    }
}
