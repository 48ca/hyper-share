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
    HttpRequest, HttpResponse, HttpStatus,
    status_to_code, status_to_message
};

const BUFFER_SIZE: usize = 4096;
// Used in the event that the request cannot be properly decoded.
const DEFAULT_HTTP_VERSION: &'static str = "HTTP/1.1";

fn write_error(error_str: String) {
    eprintln!("An error occurred: {}", error_str);
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

fn write_error_response(status: HttpStatus, stream: &TcpStream) {
    let body = format!("<html><body><h1>{} {}</h1></body></html>",
                       status_to_code(&status),
                       status_to_message(&status));
    let mut bytes = body.as_bytes();
    let mut resp = HttpResponse::new(status, DEFAULT_HTTP_VERSION, Some(&mut bytes));
    resp.set_content_length(body.len());
    let _ = resp.write_to_stream(&stream);
}

fn write_response(req: &HttpRequest, content: &mut dyn io::Read, size: usize, stream: &TcpStream) -> Result<(), io::Error> {
    let mut resp = HttpResponse::new(HttpStatus::OK, req.version_str, Some(content));
    resp.set_content_length(size);
    resp.add_header("Connection".to_string(),
                    if req.version_str != "HTTP/1.0" { "keep-alive".to_string() }
                    else { "close".to_string() });

    resp.write_to_stream(stream)
}

fn handle_new_connection(mut stream: &TcpStream) -> Result<(), io::Error> {
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

    let mut buffer: [u8; BUFFER_SIZE] = [0; BUFFER_SIZE];
    loop {
        let mut total_bytes_read: usize = 0;
        // TODO: Remove try! here
        while total_bytes_read < BUFFER_SIZE && !end_of_http_request(&mut buffer[..total_bytes_read]) {
            let bytes_read = match stream.read(&mut buffer[total_bytes_read..]) {
                Ok(size) => size,
                Err(err) => {
                    write_error(format!(
                        "Failed to read bytes from socket: {}", err));
                    // Even though the server has run into a problem, because it is
                    // a problem inherent to the socket connection, we return Ok
                    // so that we do not write an HTTP error response to the socket.
                    return Ok(());
                }
            };

            if bytes_read == 0 { break; }
            total_bytes_read += bytes_read;
        }

        let body = &buffer[..total_bytes_read];

        let req: HttpRequest = match decode_request(body) {
            Ok(r) => r,
            Err(status) => {
                write_error_response(status, stream);
                return Ok(());
            }
        };

        if req.method.is_none() {
            write_error_response(HttpStatus::NotImplemented, stream);
            return Ok(());
        }

        let canonical_path = fs::canonicalize(req.path)?;
        let metadata = match fs::metadata(&canonical_path) {
            Err(error) => {
                // Attempt to convert the system error into an HTTP error
                // that we can send back to the user.
                let kind = error.kind();
                if kind == io::ErrorKind::NotFound {
                    write_error_response(HttpStatus::NotFound, stream);
                    return Ok(());
                }
                // If we can't easily convert this error into an HTTP error,
                // just send a 500 back to the user.
                return Err(error);
            }
            Ok(data) => data
        };

        if !metadata.is_file() && !metadata.is_dir() {
            write_error_response(HttpStatus::PermissionDenied, stream);
            return Ok(());
        }

        let write_result =
            if metadata.is_file() {
                let mut file = fs::File::open(&canonical_path)?;
                let size = metadata.len();
                write_response(&req, &mut file, size as usize, stream)
            } else {
                let content = "<html><body>Directory listing isn't implemented yet!</body></html>";
                let mut body = content.as_bytes();
                let size = content.len();
                write_response(&req, &mut body, size, stream)
            };

        // If the HTTP version is >=1.1, we can leave the connection open.
        if !write_result.is_ok() || req.version_str == "HTTP/1.0" {
            break;
        }
    }

    return Ok(());
}

fn main() {
    let port: u16 = 8080;
    let mask: &'static str = "0.0.0.0";
    let listener = TcpListener::bind(format!("{mask}:{port}", mask=mask, port=port)).unwrap();
    for _stream in listener.incoming() {
        match _stream {
            Ok(stream) => {
                match handle_new_connection(&stream) {
                    Ok(_) => {},
                    Err(e) => {
                        write_error_response(HttpStatus::ServerError, &stream);
                        write_error(format!("Server error: {}", e));
                    }
                };
            }
            Err(e) => write_error(e.to_string()),
        }
    }
}
