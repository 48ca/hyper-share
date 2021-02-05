extern crate boyer_moore_magiclen;

use boyer_moore_magiclen::BMByte;

pub mod types;

use types::BMBuf;

pub fn find_body_start(buffer: &[u8]) -> Option<usize> {
    lazy_static! {
        static ref BODY_DELIM: BMByte = BMByte::from("\r\n\r\n").unwrap();
    };

    let vec = BODY_DELIM.find_in(BMBuf(buffer), 1);
    if vec.len() < 1 {
        None
    } else {
        Some(vec[0] + 4)
    }
}
