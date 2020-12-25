use std::net::TcpListener;
use std::net::TcpStream;
use std::net::SocketAddr::{V4,V6};

use std::io;
use std::io::Read;

use std::str::from_utf8;

use std::fs;

use std::format;

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

fn write_error_response(status: HttpStatus, stream: &TcpStream) -> Result<(), io::Error> {
    let body = format!("<html><body><h1>{} {}</h1></body></html>",
                       status_to_code(&status),
                       status_to_message(&status));
    let mut bytes = body.as_bytes();
    let mut resp = HttpResponse::new(status, HttpVersion::Http1_0);
    resp.set_content_length(body.len());
    resp.write_to_stream(&mut bytes, &stream)
}

enum ConnectionState {
    ReadingRequest,
    RequestReady,
    WritingResponse,
    Closing
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

struct HttpConnection<'a> {
    pub stream: &'a TcpStream,
    pub state: ConnectionState,

    // Buffer for holding a pending request
    pub buffer: [u8; BUFFER_SIZE],
    pub bytes_read: usize,

    // Space to store a per-request string response
    pub response_data: ResponseDataType,
    pub response: Option<HttpResponse>,

    pub keep_alive: bool,
}

impl HttpConnection<'_> {
    pub fn new(stream: &TcpStream) -> HttpConnection {
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
    Ok(
        if bytes_read == 0 || end_of_http_request(&buffer[..conn.bytes_read]) {
            ConnectionState::RequestReady
        } else {
            ConnectionState::ReadingRequest
        }
    )
}

fn handle_request(mut conn: &mut HttpConnection) -> Result<ConnectionState, io::Error> {
    let body = &mut conn.buffer[..conn.bytes_read];

    let req: HttpRequest = match decode_request(body) {
        Ok(r) => r,
        Err(status) => {
            write_error_response(status, conn.stream)?;
            return Ok(ConnectionState::Closing);
        }
    };

    // Check if keep-alive header was given in the request.
    // If it was not, assume keep-alive is >= HTTP/1.1.
    conn.keep_alive = match req.get_header("connection") {
        Some(value) => value.to_lowercase() == "keep-alive",
        None => req.version != HttpVersion::Http1_0
    };

    if req.method.is_none() {
        write_error_response(HttpStatus::NotImplemented, conn.stream)?;
        return Ok(ConnectionState::Closing);
    }

    let canonical_path = match fs::canonicalize(req.path) {
        Err(error) => {
            // Attempt to convert the system error into an HTTP error
            // that we can send back to the user.
            match resolve_io_error(&error) {
                Some(http_error) => write_error_response(http_error, &conn.stream)?,
                None => { return Err(error); }
            }
            return Ok(ConnectionState::Closing);
        }
        Ok(path) => path
    };

    let metadata = match fs::metadata(&canonical_path) {
        Err(error) => {
            match resolve_io_error(&error) {
                Some(http_error) => write_error_response(http_error, &conn.stream)?,
                None => { return Err(error); }
            }
            return Ok(ConnectionState::Closing);
        }
        Ok(data) => data
    };

    if !metadata.is_file() && !metadata.is_dir() {
        write_error_response(HttpStatus::PermissionDenied, &conn.stream)?;
        return Ok(ConnectionState::Closing);
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
            return Ok(ConnectionState::ReadingRequest);
        } else {
            return Ok(ConnectionState::Closing);
        }
    }

    return Ok(ConnectionState::WritingResponse);
}

fn create_http_connection(stream: &TcpStream) -> HttpConnection {
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

    HttpConnection::new(&stream)
}

fn handle_conn(mut conn: HttpConnection) -> Result<(), io::Error> {
    loop {
        match conn.state {
            ConnectionState::ReadingRequest => {
                conn.state = read_partial_request(&mut conn)?;
            }
            ConnectionState::RequestReady => {
                conn.state = handle_request(&mut conn)?;
            }
            ConnectionState::WritingResponse => {
                conn.state = write_partial_response(&mut conn)?;
            }
            ConnectionState::Closing => { break; }
        }
    }

    Ok(())
}

fn main() {
    let port: u16 = 8080;
    let mask: &'static str = "0.0.0.0";
    let listener = TcpListener::bind(format!("{mask}:{port}", mask=mask, port=port)).unwrap();
    for _stream in listener.incoming() {
        match _stream {
            Ok(stream) => {
                let conn = create_http_connection(&stream);
                match handle_conn(conn) {
                    Ok(_) => println!("Connection closing normally"),
                    Err(e) => {
                        // Ignore the return value of write_error_response, because
                        // we're closing the connection anyway.
                        let _ = write_error_response(HttpStatus::ServerError, &stream);
                        println!("Server error: {}", e);
                    }
                }
            }
            Err(e) => write_error(e.to_string()),
        }
    }
}
