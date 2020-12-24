use std::net::TcpListener;
use std::net::TcpStream;
use std::format;
use std::net::SocketAddr::{V4,V6};
use std::io::{Read,Write};
use std::str::from_utf8;

mod simple_http;
use simple_http::{HttpRequest,HttpResponse,HttpStatus,HttpHeader};

fn write_error(error_str: String) {
    eprintln!("An error occurred: {}", error_str);
}

const BUFFER_SIZE: usize = 4096;
// Used in the event that the request cannot be properly decoded.
const DEFAULT_HTTP_VERSION: &'static str = "1.1";

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

fn handle_new_connection(mut stream: TcpStream) {
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
    let mut total_bytes_read: usize = 0;
    // TODO: Remove try! here
    while total_bytes_read < BUFFER_SIZE && !end_of_http_request(&mut buffer[..total_bytes_read]) {
        let bytes_read = match stream.read(&mut buffer[total_bytes_read..]) {
            Ok(size) => size,
            Err(err) => {
                return write_error(format!(
                    "Failed to read bytes from socket: {}", err));
            }
        };

        if bytes_read == 0 { break; }
        total_bytes_read += bytes_read;
    }

    let body = &buffer[..total_bytes_read];

    let req: HttpRequest = match decode_request(body) {
        Ok(r) => r,
        Err(status) => {
            HttpResponse::new(status, DEFAULT_HTTP_VERSION, None).write_to_stream(&stream);
            return;
        }
    };


    let mut resp = HttpResponse::new(HttpStatus::OK, req.version(), None);
    resp.write_to_stream(&stream);

    return;
}

fn main() {
    let port: u16 = 8080;
    let mask: &'static str = "0.0.0.0";
    let listener = TcpListener::bind(format!("{mask}:{port}", mask=mask, port=port)).unwrap();
    for _stream in listener.incoming() {
        match _stream {
            Ok(stream) => handle_new_connection(stream),
            Err(e) => write_error(e.to_string()),
        }
    }
}
