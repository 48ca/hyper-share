use std::io;
use std::io::Write;
use std::net::TcpStream;

pub enum HttpStatus {
    OK,                      // 200
    BadRequest,              // 401
    PermissionDenied,        // 403
    NotFound,                // 404
    RequestHeadersTooLarge,  // 431
    NotImplemented,          // 501
    HttpVersionNotSupported, // 505
}

fn status_to_code(status: &HttpStatus) -> u16 {
    match status {
        HttpStatus::OK                      => 200,
        HttpStatus::BadRequest              => 401,
        HttpStatus::PermissionDenied        => 403,
        HttpStatus::NotFound                => 404,
        HttpStatus::RequestHeadersTooLarge  => 431,
        HttpStatus::NotImplemented          => 501,
        HttpStatus::HttpVersionNotSupported => 505
    }
}

fn status_to_message(status: &HttpStatus) -> &'static str {
    match status {
        HttpStatus::OK                      => "OK",
        HttpStatus::BadRequest              => "Bad request",
        HttpStatus::PermissionDenied        => "Permission denied",
        HttpStatus::NotFound                => "Not found",
        HttpStatus::RequestHeadersTooLarge  => "Request header fields too large",
        HttpStatus::NotImplemented          => "Method not implemented",
        HttpStatus::HttpVersionNotSupported => "HTTP version not supported"
    }
}

pub struct HttpRequest<'a> {
    method: &'a str,
    path: &'a str,
    version_str: &'a str,
}

impl HttpRequest<'_> {
    pub fn version(&self) -> &str {
        self.version_str
    }

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

        if verb != "GET" && verb != "HEAD" {
            return Err(HttpStatus::NotImplemented);
        }

        if version_str != "HTTP/1.0" && version_str != "HTTP/1.1" {
            return Err(HttpStatus::HttpVersionNotSupported);
        }

        // unwrap safe because we know that lines will have a last element
        if lines.last().unwrap().len() != 0 {
            // We never received the end of the request
            return Err(HttpStatus::RequestHeadersTooLarge);
        }

        Ok(HttpRequest {
            path: path,
            method: verb,
            version_str: version_str,
        })
    }
}

pub struct HttpHeader {
}

pub struct HttpResponse<'a, 'b> {
    status: HttpStatus,
    version_str: &'a str,
    headers: Vec<HttpHeader>,
    body: Option<&'b mut dyn io::Read>,
}

impl HttpResponse<'_, '_> {
    pub fn new<'a, 'b>(status: HttpStatus, version: &'a str, body: Option<&'b mut dyn io::Read>) -> HttpResponse<'a, 'b> {
        HttpResponse {
            status: status,
            version_str: version,
            headers: Vec::<HttpHeader>::new(),
            body: body,
        }
    }

    fn write_fully(buffer: &[u8], mut stream: &TcpStream) -> Result<(), io::Error> {
        let amt_to_write: usize = buffer.len();
        let mut amt_written: usize = 0;
        while amt_written < amt_to_write {
            amt_written += stream.write(&buffer[amt_written..amt_to_write])?;
        }

        Ok(())
    }

    pub fn write_to_stream(&mut self, mut stream: &TcpStream) -> Result<(), io::Error> {
        let code = status_to_code(&self.status);
        let message = status_to_message(&self.status);
        let header = format!("{version} {code} {message}\r\n\r\n",
                             version=self.version_str, code=code,
                             message=message);

        stream.write(header.as_bytes())?;

        match &mut self.body {
            Some(body) => {
                let mut buffer: [u8; 4096] = [0; 4096];
                loop {
                    let amt_read = body.read(&mut buffer)?;
                    if amt_read == 0 { break; }
                    HttpResponse::write_fully(&buffer[..amt_read], stream);
                };
            }
            None => {
                let body = format!("<html><body><h1>{} {}</h1></body></html>",
                                   code, message);
                HttpResponse::write_fully(body.as_bytes(), stream);
            }
        }

        Ok(())
    }
}
