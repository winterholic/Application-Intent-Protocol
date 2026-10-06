use crate::diag::{Diagnostic, Span};
use aip_ir::codes;

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Decimal(String),
    Str(String),
    /// Number + unit: `30s`, `10m`, `2h`, `7d`, `2w`, `6mo`, `1y`.
    Duration(i64, DurUnit),
    /// Number + size unit: `10MB`.
    Size(u64),
    TimeOfDay(u8, u8),
    Regex(String),
    Punct(&'static str),
    Eof,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub enum DurUnit {
    Sec,
    Min,
    Hour,
    Day,
    Week,
    Month,
    Year,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

const PUNCT3: [&str; 1] = ["..."];
const PUNCT2: [&str; 7] = ["!=", "<=", ">=", "+=", "-=", "->", ".."];
const PUNCT1: [&str; 20] = ["{", "}", "(", ")", "[", "]", ":", ",", ".", "=", "<", ">", "+", "-", "*", "/", "|", "?", "%", "&"];

pub fn lex(src: &str) -> Result<Vec<Token>, Diagnostic> {
    Lexer { src: src.as_bytes(), text: src, pos: 0, line: 1, col: 1, out: Vec::new() }.run()
}

struct Lexer<'a> {
    src: &'a [u8],
    text: &'a str,
    pos: usize,
    line: u32,
    col: u32,
    out: Vec<Token>,
}

impl Lexer<'_> {
    fn peek(&self, k: usize) -> u8 {
        self.src.get(self.pos + k).copied().unwrap_or(0)
    }

    fn bump(&mut self, n: usize) {
        for _ in 0..n {
            if self.peek(0) == b'\n' {
                self.line += 1;
                self.col = 1;
            } else if self.peek(0) & 0xC0 != 0x80 {
                // count columns in characters, not UTF-8 continuation bytes
                self.col += 1;
            }
            self.pos += 1;
        }
    }

    fn here(&self) -> Span {
        Span { start: self.pos as u32, end: self.pos as u32, line: self.line, col: self.col }
    }

    fn push(&mut self, tok: Tok, start: Span) {
        let span = Span { end: self.pos as u32, ..start };
        self.out.push(Token { tok, span });
    }

    fn prev_is_ident(&self, word: &str) -> bool {
        matches!(self.out.last(), Some(Token { tok: Tok::Ident(w), .. }) if w == word)
    }

    fn run(mut self) -> Result<Vec<Token>, Diagnostic> {
        while self.pos < self.src.len() {
            let c = self.peek(0);
            if c == b' ' || c == b'\t' || c == b'\r' || c == b'\n' {
                self.bump(1);
                continue;
            }
            if c == b'/' && self.peek(1) == b'/' {
                while self.pos < self.src.len() && self.peek(0) != b'\n' {
                    self.bump(1);
                }
                continue;
            }
            let start = self.here();
            if c.is_ascii_alphabetic() || c == b'_' {
                let s = self.pos;
                while self.peek(0).is_ascii_alphanumeric() || self.peek(0) == b'_' {
                    self.bump(1);
                }
                let word = self.text[s..self.pos].to_string();
                self.push(Tok::Ident(word), start);
                continue;
            }
            if c.is_ascii_digit() {
                self.number(start)?;
                continue;
            }
            if c == b'"' {
                self.string(start)?;
                continue;
            }
            if c == b'/' && self.prev_is_ident("matches") {
                self.regex(start)?;
                continue;
            }
            if let Some(p) = self.punct() {
                self.bump(p.len());
                self.push(Tok::Punct(p), start);
                continue;
            }
            let ch = self.text[self.pos..].chars().next().unwrap_or('?');
            return Err(Diagnostic::error(codes::E100, format!("unexpected character '{ch}'"), start));
        }
        let end = self.here();
        self.out.push(Token { tok: Tok::Eof, span: end });
        Ok(self.out)
    }

    fn punct(&self) -> Option<&'static str> {
        let rest = &self.text[self.pos..];
        PUNCT3.iter().chain(PUNCT2.iter()).chain(PUNCT1.iter()).find(|p| rest.starts_with(**p)).copied()
    }

    fn number(&mut self, start: Span) -> Result<(), Diagnostic> {
        let s = self.pos;
        while self.peek(0).is_ascii_digit() {
            self.bump(1);
        }
        let digits = &self.text[s..self.pos];
        // hh:mm time of day
        if digits.len() == 2
            && self.peek(0) == b':'
            && self.peek(1).is_ascii_digit()
            && self.peek(2).is_ascii_digit()
            && !self.peek(3).is_ascii_alphanumeric()
        {
            let h: u8 = digits.parse().unwrap_or(0);
            let m: u8 = self.text[self.pos + 1..self.pos + 3].parse().unwrap_or(0);
            self.bump(3);
            if h > 23 || m > 59 {
                return Err(Diagnostic::error(codes::E100, format!("invalid time of day {h:02}:{m:02}"), start));
            }
            self.push(Tok::TimeOfDay(h, m), start);
            return Ok(());
        }
        // decimal (but not a range `1..10`)
        if self.peek(0) == b'.' && self.peek(1).is_ascii_digit() {
            self.bump(1);
            while self.peek(0).is_ascii_digit() {
                self.bump(1);
            }
            let text = self.text[s..self.pos].to_string();
            self.push(Tok::Decimal(text), start);
            return Ok(());
        }
        let n: i64 = digits.parse().map_err(|_| Diagnostic::error(codes::E100, format!("integer literal too large: {digits}"), start))?;
        // unit suffix: letters directly attached
        let us = self.pos;
        while self.peek(0).is_ascii_alphabetic() {
            self.bump(1);
        }
        let unit = &self.text[us..self.pos];
        if unit.is_empty() {
            self.push(Tok::Int(n), start);
            return Ok(());
        }
        if self.peek(0).is_ascii_digit() || self.peek(0) == b'_' {
            return Err(Diagnostic::error(codes::E100, format!("malformed literal '{}'", &self.text[s..=self.pos]), start));
        }
        let tok = match unit {
            "s" => Tok::Duration(n, DurUnit::Sec),
            "m" => Tok::Duration(n, DurUnit::Min),
            "h" => Tok::Duration(n, DurUnit::Hour),
            "d" => Tok::Duration(n, DurUnit::Day),
            "w" => Tok::Duration(n, DurUnit::Week),
            "mo" => Tok::Duration(n, DurUnit::Month),
            "y" => Tok::Duration(n, DurUnit::Year),
            "KB" => Tok::Size(n as u64 * 1024),
            "MB" => Tok::Size(n as u64 * 1024 * 1024),
            "GB" => Tok::Size(n as u64 * 1024 * 1024 * 1024),
            _ => {
                return Err(Diagnostic::error(codes::E100, format!("unknown unit '{unit}' in '{}'", &self.text[s..self.pos]), start)
                    .with_help("durations: 30s 10m 2h 7d 2w 6mo 1y; sizes: 512KB 10MB 1GB"));
            }
        };
        self.push(tok, start);
        Ok(())
    }

    fn string(&mut self, start: Span) -> Result<(), Diagnostic> {
        self.bump(1);
        let mut out = String::new();
        loop {
            match self.peek(0) {
                0 | b'\n' => return Err(Diagnostic::error(codes::E100, "unterminated string", start)),
                b'"' => {
                    self.bump(1);
                    break;
                }
                b'\\' => {
                    let esc = self.peek(1);
                    out.push(match esc {
                        b'n' => '\n',
                        b't' => '\t',
                        b'"' => '"',
                        b'\\' => '\\',
                        _ => return Err(Diagnostic::error(codes::E100, "unknown escape in string", self.here())),
                    });
                    self.bump(2);
                }
                _ => {
                    let ch = self.text[self.pos..].chars().next().unwrap_or('?');
                    out.push(ch);
                    self.bump(ch.len_utf8());
                }
            }
        }
        self.push(Tok::Str(out), start);
        Ok(())
    }

    fn regex(&mut self, start: Span) -> Result<(), Diagnostic> {
        self.bump(1);
        let s = self.pos;
        while self.peek(0) != b'/' {
            if self.peek(0) == 0 || self.peek(0) == b'\n' {
                return Err(Diagnostic::error(codes::E100, "unterminated regex", start));
            }
            if self.peek(0) == b'\\' {
                self.bump(1);
            }
            self.bump(1);
        }
        let body = self.text[s..self.pos].to_string();
        self.bump(1);
        self.push(Tok::Regex(body), start);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn toks(s: &str) -> Vec<Tok> {
        lex(s).expect("lex").into_iter().map(|t| t.tok).collect()
    }

    #[test]
    fn literals() {
        assert_eq!(
            toks("10m 6mo 00:30 10MB 1.5 7 ..."),
            vec![
                Tok::Duration(10, DurUnit::Min),
                Tok::Duration(6, DurUnit::Month),
                Tok::TimeOfDay(0, 30),
                Tok::Size(10 * 1024 * 1024),
                Tok::Decimal("1.5".into()),
                Tok::Int(7),
                Tok::Punct("..."),
                Tok::Eof
            ]
        );
    }

    #[test]
    fn range_is_not_decimal() {
        assert_eq!(toks("1..30"), vec![Tok::Int(1), Tok::Punct(".."), Tok::Int(30), Tok::Eof]);
    }

    #[test]
    fn regex_only_after_matches() {
        assert_eq!(toks("matches /a\\/b/"), vec![Tok::Ident("matches".into()), Tok::Regex("a\\/b".into()), Tok::Eof]);
    }

    #[test]
    fn bad_unit() {
        assert!(lex("3 days").is_ok());
        let e = lex("3days").expect_err("unit");
        assert_eq!(e.code, "AIP-E100");
    }

    #[test]
    fn positions_count_chars() {
        let t = lex("// 한글\n  x").expect("lex");
        assert_eq!((t[0].span.line, t[0].span.col), (2, 3));
    }
}
