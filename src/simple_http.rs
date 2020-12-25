use std::io;
use std::io::Write;
use std::net::TcpStream;

#[derive(PartialEq)]
pub enum HttpMethod {
    GET,
    HEAD,
}

#[derive(PartialEq)]
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

pub enum HttpStatus {
    OK,                      // 200
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

pub struct HttpRequest<'a> {
    pub path: &'a str,
    pub method: Option<HttpMethod>,
    pub version: HttpVersion,
    headers: HttpHeaderSet,
}

impl HttpRequest<'_> {
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
            path: path,
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

pub struct HttpResponse {
    status: HttpStatus,
    version: HttpVersion,
    headers: HttpHeaderSet,
    headers_written: bool,
}

impl HttpResponse {
    pub fn new(status: HttpStatus, version: HttpVersion) -> HttpResponse {
        HttpResponse {
            status: status,
            version: version,
            headers: HttpHeaderSet::new(),
            headers_written: false,
        }
    }

    pub fn add_header(&mut self, key: String, value: String) {
        self.headers.push(HttpHeader{ key: key, value: value });
    }

    /*
    pub fn set_content<'a>(&mut self, body: &'a mut dyn io::Read) {
        self.body = Some(body);
    }
    */

    pub fn set_content_length(&mut self, size: usize) {
        self.headers.push(HttpHeader{ key: "Content-Length".to_string(), value: size.to_string() });
    }

    fn write_fully(buffer: &[u8], mut stream: &TcpStream) -> Result<(), io::Error> {
        let amt_to_write: usize = buffer.len();
        let mut amt_written: usize = 0;
        while amt_written < amt_to_write {
            amt_written += stream.write(&buffer[amt_written..amt_to_write])?;
        }

        Ok(())
    }

    pub fn write_headers_to_stream(&mut self, mut stream: &TcpStream) -> Result<(), io::Error> {
        println!("Writing headers to stream");
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
    pub fn write_to_stream(&mut self, body: &mut dyn io::Read, stream: &TcpStream) -> Result<(), io::Error> {
        self.write_headers_to_stream(stream)?;
        while !self.partial_write_to_stream(body, stream)? {};
        Ok(())
    }

    pub fn partial_write_to_stream(&mut self, body: &mut dyn io::Read, stream: &TcpStream) -> Result<bool, io::Error> {
        println!("Doing partial write");
        assert_eq!(self.headers_written, true);
        let mut buffer: [u8; 4096] = [0; 4096];
        let amt_read = body.read(&mut buffer)?;
        if amt_read == 0 { return Ok(true); }
        HttpResponse::write_fully(&buffer[..amt_read], stream)?;
        Ok(false)
    }
}
