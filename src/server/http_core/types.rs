use std::fs;
use std::io::{self, Read, Seek, SeekFrom};

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
    String(SeekableString),
    File(fs::File),
    None,
}
