//! Byte-level pass that hands the csv crate the fields univocity would see.
//!
//! The canonical validator parses with univocity configured as in
//! `CsvFile.createDefaultParserSettings()`: whitespace around an unquoted
//! value is dropped (`ignoreLeading/TrailingWhitespaces`, the default), a
//! quote that follows such whitespace still opens a quoted value, and
//! whitespace inside quotes is kept (`ignoreLeading/TrailingWhitespacesInQuotes`
//! set to false). RFC 4180 readers such as the csv crate only recognise a
//! quote in the first byte of a field, so `817, "DUS"` yields the literal
//! `"DUS"` with quotes and a leading space, and `leading_or_trailing_whitespaces`
//! cannot tell "space inside quotes" (reported by Java) from "space around a
//! bare field" (stripped by Java before validation).
//!
//! This pass rewrites the bytes once, before parsing, so the csv crate agrees
//! with univocity on both counts:
//!
//! * whitespace (any byte `<= 0x20` other than `\r` and `\n`) at the start
//!   of a field and at the end of an unquoted field is removed;
//! * whitespace between a closing quote and the next delimiter is removed;
//! * everything inside quotes, `""` escapes included, is copied verbatim.
//!
//! After the pass, a field that still starts or ends with whitespace was
//! quoted in the source, which is exactly when Java reports it. Bytes are only
//! ever removed, never reordered or inserted, so line numbers and UTF-8
//! validity are preserved. A line made only of whitespace keeps its
//! whitespace: the csv crate would otherwise drop the empty line, and the
//! `--thorough` `empty_row` notice needs to see the record.

use std::borrow::Cow;
use std::io::{self, Read};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum State {
    /// Before the first significant byte of a field.
    FieldStart,
    /// Inside a bare field; trailing whitespace is held back in `pending`.
    Unquoted,
    /// Inside a quoted field.
    Quoted,
    /// Just saw a `"` inside a quoted field; it was either an escape or the
    /// closing quote, decided by the next byte.
    QuotedQuote,
    /// After the closing quote, before the delimiter.
    AfterQuoted,
}

#[inline]
fn is_ws(b: u8) -> bool {
    b <= b' ' && b != b'\n' && b != b'\r'
}

#[inline]
fn is_line_end(b: u8) -> bool {
    b == b'\n' || b == b'\r'
}

/// Incremental normaliser; feed bytes in any chunking, then call `finish`.
#[derive(Debug)]
pub struct Normalizer {
    state: State,
    /// Whitespace not yet known to be leading, trailing, or content.
    pending: Vec<u8>,
    /// True until the first significant byte of the current line.
    at_line_start: bool,
}

impl Default for Normalizer {
    fn default() -> Self {
        Self::new()
    }
}

impl Normalizer {
    pub fn new() -> Self {
        Self {
            state: State::FieldStart,
            pending: Vec::new(),
            at_line_start: true,
        }
    }

    pub fn push(&mut self, input: &[u8], out: &mut Vec<u8>) {
        let mut i = 0;
        let n = input.len();
        while i < n {
            let b = input[i];
            match self.state {
                State::FieldStart => {
                    if is_ws(b) {
                        self.pending.push(b);
                        i += 1;
                    } else if is_line_end(b) {
                        // A whitespace-only line stays a one-field record.
                        if self.at_line_start {
                            out.extend_from_slice(&self.pending);
                        }
                        self.pending.clear();
                        out.push(b);
                        self.at_line_start = true;
                        i += 1;
                    } else {
                        self.pending.clear();
                        self.at_line_start = false;
                        if b == b'"' {
                            out.push(b);
                            self.state = State::Quoted;
                        } else if b == b',' {
                            out.push(b);
                        } else {
                            out.push(b);
                            self.state = State::Unquoted;
                        }
                        i += 1;
                    }
                }
                State::Unquoted => {
                    // Copy the run of ordinary bytes in one go.
                    let start = i;
                    while i < n && !(input[i] <= b' ' || input[i] == b',') {
                        i += 1;
                    }
                    if i > start {
                        out.extend_from_slice(&self.pending);
                        self.pending.clear();
                        out.extend_from_slice(&input[start..i]);
                    }
                    if i < n {
                        let b = input[i];
                        if is_ws(b) {
                            self.pending.push(b);
                        } else {
                            // Delimiter or line end: trailing whitespace goes.
                            self.pending.clear();
                            out.push(b);
                            self.state = State::FieldStart;
                            self.at_line_start = is_line_end(b);
                        }
                        i += 1;
                    }
                }
                State::Quoted => {
                    let start = i;
                    while i < n && input[i] != b'"' {
                        i += 1;
                    }
                    out.extend_from_slice(&input[start..i]);
                    if i < n {
                        out.push(b'"');
                        self.state = State::QuotedQuote;
                        i += 1;
                    }
                }
                State::QuotedQuote => {
                    if b == b'"' {
                        out.push(b);
                        self.state = State::Quoted;
                        i += 1;
                    } else {
                        // The previous quote closed the field; reprocess `b`.
                        self.state = State::AfterQuoted;
                    }
                }
                State::AfterQuoted => {
                    if is_ws(b) {
                        // univocity skips whitespace after the closing quote.
                    } else if b == b',' || is_line_end(b) {
                        out.push(b);
                        self.state = State::FieldStart;
                        self.at_line_start = is_line_end(b);
                    } else {
                        // Stray bytes after a closing quote: both parsers keep
                        // them, in slightly different ways; pass them through.
                        out.push(b);
                    }
                    i += 1;
                }
            }
        }
    }

    pub fn finish(&mut self, out: &mut Vec<u8>) {
        if self.state == State::FieldStart && self.at_line_start {
            // Whitespace-only last line without a newline.
            out.extend_from_slice(&self.pending);
        }
        self.pending.clear();
        self.state = State::FieldStart;
        self.at_line_start = true;
    }
}

/// True when `data` has a byte the pass could remove. Cheap pre-check so a
/// clean table is parsed from the original buffer without a copy.
pub fn needs_normalization(data: &[u8]) -> bool {
    data.iter().any(|&b| is_ws(b))
}

/// Normalise a whole buffer. Borrowed when nothing would change.
pub fn normalize(data: &[u8]) -> Cow<'_, [u8]> {
    if !needs_normalization(data) {
        return Cow::Borrowed(data);
    }
    let mut out = Vec::with_capacity(data.len());
    let mut normalizer = Normalizer::new();
    normalizer.push(data, &mut out);
    normalizer.finish(&mut out);
    Cow::Owned(out)
}

/// `Read` adapter applying the same pass to a stream.
pub struct NormalizingReader<R> {
    inner: R,
    normalizer: Normalizer,
    input: Vec<u8>,
    output: Vec<u8>,
    cursor: usize,
    eof: bool,
}

impl<R: Read> NormalizingReader<R> {
    const CHUNK: usize = 64 * 1024;

    pub fn new(inner: R) -> Self {
        Self {
            inner,
            normalizer: Normalizer::new(),
            input: vec![0; Self::CHUNK],
            output: Vec::with_capacity(Self::CHUNK),
            cursor: 0,
            eof: false,
        }
    }

    pub fn into_inner(self) -> R {
        self.inner
    }

    fn refill(&mut self) -> io::Result<()> {
        self.output.clear();
        self.cursor = 0;
        while self.output.is_empty() && !self.eof {
            let read = self.inner.read(&mut self.input)?;
            if read == 0 {
                self.eof = true;
                self.normalizer.finish(&mut self.output);
            } else {
                self.normalizer.push(&self.input[..read], &mut self.output);
            }
        }
        Ok(())
    }
}

impl<R: Read> Read for NormalizingReader<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if self.cursor >= self.output.len() {
            self.refill()?;
            if self.output.is_empty() {
                return Ok(0);
            }
        }
        let available = &self.output[self.cursor..];
        let count = available.len().min(buf.len());
        buf[..count].copy_from_slice(&available[..count]);
        self.cursor += count;
        Ok(count)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn norm(input: &str) -> String {
        String::from_utf8(normalize(input.as_bytes()).into_owned()).unwrap()
    }

    fn norm_stream(input: &str, chunk: usize) -> String {
        struct Chunked<'a>(&'a [u8], usize);
        impl Read for Chunked<'_> {
            fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
                let n = self.0.len().min(self.1).min(buf.len());
                buf[..n].copy_from_slice(&self.0[..n]);
                self.0 = &self.0[n..];
                Ok(n)
            }
        }
        let mut out = String::new();
        NormalizingReader::new(Chunked(input.as_bytes(), chunk))
            .read_to_string(&mut out)
            .unwrap();
        out
    }

    #[test]
    fn clean_input_is_borrowed() {
        let data = b"a,b\n1,2\n";
        assert!(matches!(normalize(data), Cow::Borrowed(_)));
    }

    #[test]
    fn space_before_quote_opens_the_quote() {
        // Latvia (mdb-992): `817, 9506, "DUS", ...`
        assert_eq!(norm("817, 9506, \"DUS\", x\n"), "817,9506,\"DUS\",x\n");
    }

    #[test]
    fn bare_fields_are_trimmed_on_both_sides() {
        assert_eq!(norm(" a , b ,c \n"), "a,b,c\n");
        assert_eq!(norm("\ta\t,\tb\n"), "a,b\n");
    }

    #[test]
    fn whitespace_inside_quotes_is_kept() {
        assert_eq!(norm("\" a \",\"b \"\n"), "\" a \",\"b \"\n");
    }

    #[test]
    fn escaped_quotes_and_delimiters_inside_quotes_survive() {
        assert_eq!(norm("\"a \"\"b\"\" , c\",d\n"), "\"a \"\"b\"\" , c\",d\n");
        assert_eq!(norm("\"line\nbreak\",x\n"), "\"line\nbreak\",x\n");
    }

    #[test]
    fn whitespace_after_closing_quote_is_dropped() {
        assert_eq!(norm("\"a\"  ,b\n\"c\" \n"), "\"a\",b\n\"c\"\n");
    }

    #[test]
    fn quote_inside_bare_field_is_literal() {
        assert_eq!(norm("ab\"c, d\"e \n"), "ab\"c,d\"e\n");
        assert_eq!(norm("a \"b\",c\n"), "a \"b\",c\n");
    }

    #[test]
    fn crlf_and_trailing_whitespace_before_line_end() {
        assert_eq!(norm("a ,b \r\nc\t\r\n"), "a,b\r\nc\r\n");
    }

    #[test]
    fn empty_fields_and_trailing_field() {
        assert_eq!(norm(" , ,\n,\n"), ",,\n,\n");
    }

    #[test]
    fn whitespace_only_lines_keep_their_record() {
        assert_eq!(norm("a,b\n   \nc,d\n"), "a,b\n   \nc,d\n");
        assert_eq!(norm("a,b\n  "), "a,b\n  ");
        assert_eq!(norm("a,b\n  \r\n"), "a,b\n  \r\n");
    }

    #[test]
    fn trailing_whitespace_at_eof_without_newline() {
        assert_eq!(norm("a,b\n1,2  "), "a,b\n1,2");
    }

    #[test]
    fn nul_and_control_bytes_count_as_whitespace() {
        assert_eq!(norm("a\u{0},\u{1}b\n"), "a,b\n");
    }

    #[test]
    fn streaming_matches_buffered_for_every_chunk_size() {
        let input = "h1, h2 ,\"h 3\"\n 1 , \"x \"\" y\" ,\"z\"  \n   \n\"a\nb\" , c\t\r\n  ";
        let expected = norm(input);
        for chunk in 1..=input.len() {
            assert_eq!(norm_stream(input, chunk), expected, "chunk size {chunk}");
        }
    }
}
