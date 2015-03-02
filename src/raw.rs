//! Raw un-exported bindings to miniz for encoding/decoding

use std::io::prelude::*;
use std::io;
use std::mem;
use std::ops::{Deref, DerefMut};
use libc;

use Compression;
use ffi;
use self::Flavor::{Deflate,Inflate};

pub struct EncoderWriter<W> {
    pub inner: Option<W>,
    stream: Stream,
    buf: Vec<u8>,
}

pub struct EncoderReader<R> {
    pub inner: R,
    stream: Stream,
    buf: Box<[u8]>,
    pos: usize,
    cap: usize,
}

pub struct DecoderReader<R> {
    pub inner: R,
    stream: Stream,
    pub pos: usize,
    pub cap: usize,
    pub buf: Box<[u8]>,
}

pub struct DecoderWriter<W> {
    pub inner: Option<W>,
    stream: Stream,
    buf: Vec<u8>,
}

enum Flavor { Deflate, Inflate }

struct Stream(ffi::mz_stream, Flavor);

impl<W: Write> EncoderWriter<W> {
    pub fn new(w: W, level: Compression, raw: bool,
               buf: Vec<u8>) -> EncoderWriter<W> {
        EncoderWriter {
            inner: Some(w),
            stream: Stream::new(Deflate, raw, level),
            buf: buf,
        }
    }

    pub fn do_finish(&mut self) -> io::Result<()> {
        let inner = self.inner.as_mut().unwrap();
        try!(self.stream.write(&[], ffi::MZ_FINISH, &mut self.buf, inner,
                               ffi::mz_deflate));
        try!(inner.write_all(&self.buf));
        self.buf.truncate(0);
        Ok(())
    }
}

impl<W: Write> Write for EncoderWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf, ffi::MZ_NO_FLUSH, &mut self.buf,
                          self.inner.as_mut().unwrap(), ffi::mz_deflate)
    }

    fn flush(&mut self) -> io::Result<()> {
        let inner = self.inner.as_mut().unwrap();
        try!(self.stream.write(&[], ffi::MZ_SYNC_FLUSH, &mut self.buf, inner,
                               ffi::mz_deflate));
        if self.buf.len() > 0 {
            try!(inner.write_all(&self.buf));
        }
        inner.flush()
    }
}

#[unsafe_destructor]
impl<W: Write> Drop for EncoderWriter<W> {
    fn drop(&mut self) {
        match self.inner {
            Some(..) => { let _ = self.do_finish(); }
            None => {}
        }
    }
}

impl<R: Read> EncoderReader<R> {
    pub fn new(w: R, level: Compression, raw: bool,
               buf: Vec<u8>) -> EncoderReader<R> {
        EncoderReader {
            inner: w,
            stream: Stream::new(Deflate, raw, level),
            buf: buf.into_boxed_slice(),
            cap: 0,
            pos: 0,
        }
    }
}

impl<R: Read> Read for EncoderReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        self.stream.read(buf, &mut self.buf, &mut self.pos, &mut self.cap,
                         &mut self.inner, ffi::mz_deflate)
    }
}

impl<R: Read> DecoderReader<R> {
    pub fn new(r: R, raw: bool, buf: Vec<u8>) -> DecoderReader<R> {
        DecoderReader {
            inner: r,
            stream: Stream::new(Inflate, raw, Compression::None),
            pos: 0,
            buf: buf.into_boxed_slice(),
            cap: 0,
        }
    }
}

impl<R: Read> Read for DecoderReader<R> {
    fn read(&mut self, into: &mut [u8]) -> io::Result<usize> {
        self.stream.read(into, &mut self.buf, &mut self.pos, &mut self.cap,
                         &mut self.inner, ffi::mz_inflate)
    }
}

impl<W: Write> DecoderWriter<W> {
    pub fn new(w: W, raw: bool, buf: Vec<u8>) -> DecoderWriter<W> {
        DecoderWriter {
            inner: Some(w),
            stream: Stream::new(Inflate, raw, Compression::None),
            buf: buf,
        }
    }

    pub fn do_finish(&mut self) -> io::Result<()> {
        let inner = self.inner.as_mut().unwrap();
        try!(self.stream.write(&[], ffi::MZ_FINISH, &mut self.buf, inner,
                               ffi::mz_inflate));
        try!(inner.write_all(&self.buf));
        self.buf.truncate(0);
        Ok(())
    }
}

impl<W: Write> Write for DecoderWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.stream.write(buf, ffi::MZ_NO_FLUSH, &mut self.buf,
                          self.inner.as_mut().unwrap(), ffi::mz_inflate)
    }

    fn flush(&mut self) -> io::Result<()> {
        let inner = self.inner.as_mut().unwrap();
        try!(self.stream.write(&[], ffi::MZ_SYNC_FLUSH, &mut self.buf, inner,
                               ffi::mz_inflate));
        if self.buf.len() > 0 {
            try!(inner.write_all(&self.buf));
        }
        inner.flush()
    }
}

impl Stream {
    fn new(kind: Flavor, raw: bool, level: Compression) -> Stream {
        let mut state: ffi::mz_stream = unsafe { mem::zeroed() };
        let ret = match kind {
            Deflate => unsafe {
                ffi::mz_deflateInit2(&mut state,
                                     level as libc::c_int,
                                     ffi::MZ_DEFLATED,
                                     if raw {
                                         -ffi::MZ_DEFAULT_WINDOW_BITS
                                     } else {
                                         ffi::MZ_DEFAULT_WINDOW_BITS
                                     },
                                     9,
                                     ffi::MZ_DEFAULT_STRATEGY)
            },
            Inflate => unsafe {
                ffi::mz_inflateInit2(&mut state,
                                     if raw {
                                         -ffi::MZ_DEFAULT_WINDOW_BITS
                                     } else {
                                         ffi::MZ_DEFAULT_WINDOW_BITS
                                     })
            }
        };
        assert_eq!(ret, 0);
        Stream(state, kind)
    }

    fn read<R: Read>(&mut self, into: &mut [u8], buf: &mut [u8],
                     pos: &mut usize, cap: &mut usize, reader: &mut R,
                     f: unsafe extern fn(*mut ffi::mz_stream,
                                         libc::c_int) -> libc::c_int)
                     -> io::Result<usize> {
        loop {
            let mut eof = false;
            if *pos == *cap {
                *cap = try!(reader.take(buf.len() as u64).read(buf));
                *pos = 0;
                eof = *cap == 0;
            }

            let next_in = &buf[*pos..*cap];

            self.next_in = next_in.as_ptr();
            self.avail_in = next_in.len() as libc::c_uint;
            self.next_out = into.as_mut_ptr();
            self.avail_out = into.len() as libc::c_uint;

            let before_out = self.total_out;
            let before_in = self.total_in;

            let flush = if eof {ffi::MZ_FINISH} else {ffi::MZ_NO_FLUSH};
            let ret = unsafe { f(&mut **self, flush) };
            let read = (self.total_out - before_out) as usize;
            *pos += (self.total_in - before_in) as usize;

            return match ret {
                ffi::MZ_OK | ffi::MZ_BUF_ERROR => {
                    // If we haven't ready any data and we haven't hit EOF yet,
                    // then we need to keep asking for more data because if we
                    // return that 0 bytes of data have been read then it will
                    // be interpreted as EOF.
                    if read == 0 && !eof { continue }
                    Ok(read)
                }
                ffi::MZ_STREAM_END => return Ok(read),
                ffi::MZ_DATA_ERROR => {
                    Err(io::Error::new(io::ErrorKind::InvalidInput,
                                       "corrupt deflate stream", None))
                }
                n => panic!("unexpected return {}", n),
            }
        }
    }

    fn write<W: Write>(&mut self, buf: &[u8], flush: libc::c_int,
                        into: &mut Vec<u8>, writer: &mut W,
                        f: unsafe extern fn(*mut ffi::mz_stream,
                                            libc::c_int) -> libc::c_int)
                        -> io::Result<usize> {
        if into.len() > 0 {
            try!(writer.write_all(into));
            into.truncate(0);
        }

        let cur_len = into.len();

        self.next_in = buf.as_ptr();
        self.avail_in = buf.len() as libc::c_uint;
        self.next_out = into[cur_len..].as_mut_ptr();
        self.avail_out = (into.capacity() - cur_len) as libc::c_uint;

        let before_out = self.total_out;
        let before_in = self.total_in;

        let ret = unsafe {
            let ret = f(&mut **self, flush);
            into.set_len(cur_len + (self.total_out - before_out) as usize);
            ret
        };
        match ret {
            ffi::MZ_OK
            | ffi::MZ_BUF_ERROR
            | ffi::MZ_STREAM_END => Ok((self.total_in - before_in) as usize),
            n => panic!("unexpected return {}", n),
        }
    }
}

impl Deref for Stream {
    type Target = ffi::mz_stream;
    fn deref<'a>(&'a self) -> &'a ffi::mz_stream {
        let Stream(ref inner, _) = *self; inner
    }
}

impl DerefMut for Stream {
    fn deref_mut<'a>(&'a mut self) -> &'a mut ffi::mz_stream {
        let Stream(ref mut inner, _) = *self; inner
    }
}

impl Drop for Stream {
    fn drop(&mut self) {
        unsafe {
            match *self {
                Stream(ref mut s, Deflate) => ffi::mz_deflateEnd(s),
                Stream(ref mut s, Inflate) => ffi::mz_inflateEnd(s),
            };
        }
    }
}
