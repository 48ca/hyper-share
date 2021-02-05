extern crate boyer_moore_magiclen;

use core::slice::Iter;

use boyer_moore_magiclen::BMByteSearchable;

pub struct BMBuf<'a>(pub &'a [u8]);

impl BMByteSearchable for BMBuf<'_> {
    #[inline]
    fn len(&self) -> usize {
        self.0.len()
    }

    #[inline]
    fn value_at(&self, index: usize) -> u8 {
        self.0[index]
    }

    #[inline]
    fn iter(&self) -> Iter<u8> {
        self.0.iter()
    }
}
