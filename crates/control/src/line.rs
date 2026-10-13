//! Bounded line reading: a hostile peer cannot make us buffer an unbounded line.

use std::io::{self, BufRead};
use std::time::Instant;

/// Outcome of [`read_line_bounded`].
#[derive(Debug, PartialEq, Eq)]
pub enum LineRead {
    /// End of input with no pending bytes.
    Eof,
    /// A complete line is in the buffer (without the line terminator).
    Line,
    /// The line exceeded the limit; the buffer holds a prefix. `line_complete` tells whether the
    /// terminating newline was already consumed (otherwise call [`discard_line`]).
    TooLong { line_complete: bool },
}

/// Appends one line (at most `max` bytes) to `buf`. On a timeout error `buf` keeps the partial
/// line, so the caller may call again. `deadline` bounds the total time spent on the line
/// (slow-drip peers); it yields `TimedOut` once passed.
pub fn read_line_bounded<R: BufRead + ?Sized>(
    reader: &mut R,
    max: usize,
    buf: &mut Vec<u8>,
    deadline: Option<Instant>,
) -> io::Result<LineRead> {
    loop {
        if deadline.is_some_and(|d| Instant::now() >= d) {
            return Err(io::Error::new(io::ErrorKind::TimedOut, "line deadline passed"));
        }
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(if buf.is_empty() { LineRead::Eof } else { finish(buf) });
        }
        match chunk.iter().position(|b| *b == b'\n') {
            Some(pos) => {
                let (head, _) = chunk.split_at(pos);
                if buf.len().saturating_add(head.len()) > max {
                    reader.consume(pos.saturating_add(1));
                    return Ok(LineRead::TooLong { line_complete: true });
                }
                buf.extend_from_slice(head);
                reader.consume(pos.saturating_add(1));
                return Ok(finish(buf));
            }
            None => {
                let n = chunk.len();
                if buf.len().saturating_add(n) > max {
                    reader.consume(n);
                    return Ok(LineRead::TooLong { line_complete: false });
                }
                buf.extend_from_slice(chunk);
                reader.consume(n);
            }
        }
    }
}

fn finish(buf: &mut Vec<u8>) -> LineRead {
    if buf.last() == Some(&b'\r') {
        buf.pop();
    }
    LineRead::Line
}

/// Consumes input up to and including the next newline (or EOF) without buffering it.
pub fn discard_line<R: BufRead + ?Sized>(reader: &mut R) -> io::Result<()> {
    loop {
        let chunk = match reader.fill_buf() {
            Ok(chunk) => chunk,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        };
        if chunk.is_empty() {
            return Ok(());
        }
        match chunk.iter().position(|b| *b == b'\n') {
            Some(pos) => {
                reader.consume(pos.saturating_add(1));
                return Ok(());
            }
            None => {
                let n = chunk.len();
                reader.consume(n);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufReader, Cursor};

    #[test]
    fn reads_lines_and_strips_cr() {
        let mut r = Cursor::new(b"ab\r\ncd\n\nlast".to_vec());
        let mut out = Vec::new();
        for expected in [&b"ab"[..], b"cd", b"", b"last"] {
            let mut buf = Vec::new();
            assert_eq!(read_line_bounded(&mut r, 100, &mut buf, None).unwrap(), LineRead::Line);
            assert_eq!(buf, expected);
            out.push(buf);
        }
        let mut buf = Vec::new();
        assert_eq!(read_line_bounded(&mut r, 100, &mut buf, None).unwrap(), LineRead::Eof);
    }

    #[test]
    fn too_long_lines_are_cut_off_and_discardable() {
        // Tiny capacity forces the "no newline in this chunk" path.
        let mut r = BufReader::with_capacity(4, Cursor::new(b"0123456789\nok\n".to_vec()));
        let mut buf = Vec::new();
        let got = read_line_bounded(&mut r, 6, &mut buf, None).unwrap();
        assert_eq!(got, LineRead::TooLong { line_complete: false });
        discard_line(&mut r).unwrap();
        buf.clear();
        assert_eq!(read_line_bounded(&mut r, 6, &mut buf, None).unwrap(), LineRead::Line);
        assert_eq!(buf, b"ok");
        let mut r = Cursor::new(b"0123456789\nx\n".to_vec());
        buf.clear();
        let got = read_line_bounded(&mut r, 6, &mut buf, None).unwrap();
        assert_eq!(got, LineRead::TooLong { line_complete: true });
    }

    #[test]
    fn passed_deadline_is_a_timeout() {
        let mut r = Cursor::new(b"abc\n".to_vec());
        let mut buf = Vec::new();
        let err = read_line_bounded(&mut r, 10, &mut buf, Some(Instant::now())).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::TimedOut);
    }
}
