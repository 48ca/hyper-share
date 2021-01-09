extern crate regex;
use regex::{Regex,Captures};

use std::io;
use std::io::Write;
use std::net::TcpStream;
use std::cmp::min;
use std::boxed::Box;
use std::mem;

#[derive(PartialEq)]
pub enum HttpMethod {
    GET,
    HEAD,
}

#[derive(PartialEq, Clone)]
pub enum HttpVersion {
    Http1_0,
    Http1_1,
}

pub fn version_to_str(v: &HttpVersion) -> &'static str {
    match v {
        HttpVersion::Http1_0 => "HTTP/1.0",
        HttpVersion::Http1_1 => "HTTP/1.1",
    }
}

pub const BUFFER_SIZE: usize = 512 * 1024;

pub enum HttpStatus {
    OK,                      // 200
    PartialContent,          // 206
    BadRequest,              // 401
    PermissionDenied,        // 403
    NotFound,                // 404
    RequestHeadersTooLarge,  // 431
    ServerError,             // 500
    NotImplemented,          // 501
    HttpVersionNotSupported, // 505
}

pub fn status_to_code(status: &HttpStatus) -> u16 {
    match status {
        HttpStatus::OK                      => 200,
        HttpStatus::PartialContent          => 206,
        HttpStatus::BadRequest              => 401,
        HttpStatus::PermissionDenied        => 403,
        HttpStatus::NotFound                => 404,
        HttpStatus::RequestHeadersTooLarge  => 431,
        HttpStatus::ServerError             => 500,
        HttpStatus::NotImplemented          => 501,
        HttpStatus::HttpVersionNotSupported => 505
    }
}

pub fn status_to_message(status: &HttpStatus) -> &'static str {
    match status {
        HttpStatus::OK                      => "OK",
        HttpStatus::PartialContent          => "Partial Content",
        HttpStatus::BadRequest              => "Bad request",
        HttpStatus::PermissionDenied        => "Permission denied",
        HttpStatus::NotFound                => "Not found",
        HttpStatus::RequestHeadersTooLarge  => "Request header fields too large",
        HttpStatus::ServerError             => "Server error",
        HttpStatus::NotImplemented          => "Method not implemented",
        HttpStatus::HttpVersionNotSupported => "HTTP version not supported"
    }
}

pub struct HttpHeader {
    key: String,
    value: String,
}

// Don't support multiple header values yet
type HttpHeaderSet = Vec<HttpHeader>;

pub struct HttpRequest {
    pub path: String,
    pub method: Option<HttpMethod>,
    pub version: HttpVersion,
    headers: HttpHeaderSet,
}

impl HttpRequest {
    pub fn new(request_str: &str) -> Result<HttpRequest, HttpStatus> {
        /* GET /path/to/file HTTP/1.1
         * Header: value
         *
         */
        let lines: Vec<&str> = request_str.split("\r\n").collect();
        // We know that lines will always be at least 2 lines long.
        let first: Vec<&str> = lines[0].split(" ").collect();
        if first.len() != 3 {
            return Err(HttpStatus::BadRequest);
        }
        let verb = first[0];
        let path = first[1];
        let version_str = first[2];

        let version = if version_str == "HTTP/1.0" {
            HttpVersion::Http1_0
        } else if version_str == "HTTP/1.1" {
            HttpVersion::Http1_1
        } else {
            return Err(HttpStatus::HttpVersionNotSupported);
        };

        // unwrap safe because we know that lines will have a last element
        if lines.last().unwrap().len() != 0 {
            // We never received the end of the request
            return Err(HttpStatus::RequestHeadersTooLarge);
        }

        let method = if verb == "GET" { Some(HttpMethod::GET) }
                     else if verb == "HEAD" { Some(HttpMethod::HEAD) }
                     else { None };

        let mut headers = HttpHeaderSet::new();
        for header_line in &lines[1..] {
            if header_line.len() == 0 { continue; }
            let keyval: Vec<&str> = header_line.split(":").collect();
            if keyval.len() != 2 { continue; }
            headers.push(HttpHeader {
                key: keyval[0].trim().to_lowercase(),
                value: keyval[1].trim().to_string()
            });
        }

        Ok(HttpRequest {
            path: undo_percent_encoding(path),
            method: method,
            version: version,
            headers: headers
        })
    }

    pub fn get_header(&self, key: &str) -> Option<&String> {
        for header in &self.headers {
            if header.key == key.to_string() {
                return Some(&header.value);
            }
        }
        None
    }
}

fn get_byte_from_hex(tens_dig: u8, ones_dig: u8) -> u8 {
    fn get_byte_from_hex_digit(dig: u8) -> u8 {
        match dig as char {
            '0'..='9' => dig - b'0',
            'a'..='z' => dig - b'a',
            'A'..='Z' => dig - b'A',
            _ => panic!("get_byte_from_hex failed: {} = `{}`", dig, dig as char),
        }
    }

    get_byte_from_hex_digit(tens_dig) << 4 +
        get_byte_from_hex_digit(ones_dig)
}

fn undo_percent_encoding(path: &str) -> String {
    lazy_static! {
        static ref RE: Regex = Regex::new("%([0-9a-fA-F])([0-9a-fA-F])").unwrap();
    }
    let s = RE.replace_all(path, |caps: &Captures| {
        let dig: u8 = get_byte_from_hex(caps[1].bytes().nth(0).unwrap(), caps[2].bytes().nth(0).unwrap());
        let dig_arr: [u8; 1] = [dig];
        String::from_utf8_lossy(&dig_arr[..]).to_string()
    });
    s.to_string()
}

pub struct HttpResponse {
    status: HttpStatus,
    version: HttpVersion,
    headers: HttpHeaderSet,
    headers_written: bool,
    last_write_length: usize,
    buffer: Box<[u8; BUFFER_SIZE]>,
    bytes_to_write: usize,
}

impl HttpResponse {
    pub fn new(status: HttpStatus, version: &HttpVersion) -> HttpResponse {
        let buf: [u8; BUFFER_SIZE] = unsafe { mem::MaybeUninit::uninit().assume_init() };
        HttpResponse {
            status: status,
            version: version.clone(),
            headers: HttpHeaderSet::new(),
            headers_written: false,
            last_write_length: BUFFER_SIZE,
            buffer: Box::new(buf),
            bytes_to_write: 0,
        }
    }

    pub fn add_header(&mut self, key: String, value: String) {
        self.headers.push(HttpHeader{ key: key, value: value });
    }

    pub fn set_content_length(&mut self, size: usize) {
        self.headers.push(HttpHeader{ key: "Content-Length".to_string(), value: size.to_string() });
        self.bytes_to_write = size;
    }

    pub fn get_code(&self) -> String {
        status_to_code(&self.status).to_string()
    }

    #[allow(dead_code)]
    fn write_fully(buffer: &[u8], mut stream: &TcpStream) -> Result<(), io::Error> {
        let amt_to_write: usize = buffer.len();
        let mut amt_written: usize = 0;
        while amt_written < amt_to_write {
            amt_written += stream.write(&buffer[amt_written..amt_to_write])?;
        }

        Ok(())
    }

    pub fn write_headers_to_stream(&mut self, mut stream: &TcpStream) -> Result<(), io::Error> {
        assert_eq!(self.headers_written, false);
        let code = status_to_code(&self.status);
        let message = status_to_message(&self.status);
        let leader = format!("{version} {code} {message}\r\n",
                             version=version_to_str(&self.version), code=code,
                             message=message);

        stream.write(leader.as_bytes())?;

        for header in &self.headers {
            stream.write(format!("{}: {}\r\n", header.key, header.value).as_bytes())?;
        }

        stream.write(b"\r\n")?;
        
        self.headers_written = true;

        Ok(())
    }

    #[allow(dead_code)]
    pub fn write_to_stream<T>(&mut self, body: &mut T, stream: &TcpStream) -> Result<(), io::Error>
    where
        T: io::Read + io::Seek
    {
        self.write_headers_to_stream(stream)?;
        while self.partial_write_to_stream(body, stream)? > 0 {};
        Ok(())
    }

    pub fn partial_write_to_stream<T>(&mut self, body: &mut T, mut stream: &TcpStream) -> Result<usize, io::Error>
    where
        T: io::Read + io::Seek
    {
        assert_eq!(self.headers_written, true);
        let write_length = min(self.bytes_to_write, BUFFER_SIZE);
        // let write_length = min(self.last_write_length + 4096, BUFFER_SIZE);
        let amt_read = body.read(&mut self.buffer[..write_length])?;
        if amt_read == 0 { return Ok(0); }
        // HttpResponse::write_fully(&buffer[..amt_read], stream)?;
        let amt_written = stream.write(&self.buffer[..amt_read])?;
        if amt_written < amt_read {
            body.seek(io::SeekFrom::Current((amt_read - amt_written) as i64))?;
        }
        self.last_write_length = amt_written;
        self.bytes_to_write -= amt_written;
        Ok(amt_written)
    }
}
