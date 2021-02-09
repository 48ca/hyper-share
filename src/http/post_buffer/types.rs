use crate::http::http_core::HttpStatus;

#[derive(Clone)]
pub struct PostBufferError {
    code: HttpStatus,
    reason: String,
}

impl PostBufferError {
    pub fn new(code: HttpStatus, reason: String) -> PostBufferError {
        PostBufferError {
            code: code,
            reason: reason,
        }
    }
    pub fn server_error(reason: String) -> PostBufferError {
        PostBufferError {
            code: HttpStatus::ServerError,
            reason: reason,
        }
    }
    pub fn no_error() -> PostBufferError {
        PostBufferError {
            code: HttpStatus::OK,
            reason: "No error occurred.".to_string(),
        }
    }
    pub fn add_error(&mut self, e: &PostBufferError) {
        if self.code == HttpStatus::OK {
            self.code = e.code.clone();
            self.reason = e.reason.clone();
        } else {
            self.reason = format!("{} {}", self.reason, e.reason.clone());
        }
    }
    pub fn get_code(&self) -> HttpStatus { self.code }
    pub fn get_reason(&self) -> &String { &self.reason }
}
