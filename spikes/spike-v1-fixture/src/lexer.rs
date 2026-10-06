use crate::diag::{Diag, Span};
use crate::limits::{check_source, token_limit, MAX_TOKENS};

#[derive(Debug, Clone, PartialEq)]
pub enum Tok {
    Ident(String),
    Int(i64),
    Str(String),
    Dur(u64),
    Sym(&'static str),
    Eof,
}

#[derive(Debug, Clone)]
pub struct Token {
    pub tok: Tok,
    pub span: Span,
}

const SYMS: [&str; 19] = ["..", "!=", ">=", "<=", "{", "}", "(", ")", ":", ";", ",", ".", "=", ">", "<", "?", "[", "+", "-"];

/// `line_base`는 호스트 파일 안 블록 시작 줄이다. E 추출 시 진단 위치를 원 파일 기준으로 맞춘다.
/// `col_base`는 첫 줄의 시작 열이다. 호스트 문자열 안 정책 식의 열 위치를 원본 기준으로 맞춘다.
pub fn lex(src: &str, line_base: u32, col_base: u32) -> Result<Vec<Token>, Diag> {
    check_source(src)?;
    let chars: Vec<char> = src.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line, mut col) = (0usize, line_base, col_base);
    while i < chars.len() {
        let c = chars[i];
        let span = Span { line, col };
        if c == '\n' {
            line += 1;
            col = 1;
            i += 1;
            continue;
        }
        if c.is_whitespace() {
            i += 1;
            col += 1;
            continue;
        }
        if c == '/' && chars.get(i + 1) == Some(&'/') {
            while i < chars.len() && chars[i] != '\n' {
                i += 1;
            }
            continue;
        }
        if c.is_ascii_digit() {
            let start = i;
            while i < chars.len() && chars[i].is_ascii_digit() {
                i += 1;
            }
            let n: i64 =
                chars[start..i].iter().collect::<String>().parse().map_err(|_| Diag::new("LEX_NUMBER_RANGE", "정수가 지원 범위를 벗어남", span))?;
            if i < chars.len() && chars[i].is_ascii_alphabetic() {
                let us = i;
                while i < chars.len() && chars[i].is_ascii_alphabetic() {
                    i += 1;
                }
                let unit: String = chars[us..i].iter().collect();
                let factor = match unit.as_str() {
                    "ms" => 1,
                    "s" => 1000,
                    "m" => 60_000,
                    _ => return Err(Diag::new("LEX_BAD_DURATION", format!("알 수 없는 시간 단위 `{unit}`"), span)),
                };
                let ms =
                    (n as u64).checked_mul(factor).ok_or_else(|| Diag::new("LEX_DURATION_RANGE", "시간이 millisecond 지원 범위를 벗어남", span))?;
                col += (i - start) as u32;
                push(&mut out, Token { tok: Tok::Dur(ms), span })?;
                continue;
            }
            col += (i - start) as u32;
            push(&mut out, Token { tok: Tok::Int(n), span })?;
            continue;
        }
        if c.is_alphabetic() || c == '_' {
            let start = i;
            while i < chars.len() && (chars[i].is_alphanumeric() || chars[i] == '_') {
                i += 1;
            }
            col += (i - start) as u32;
            push(&mut out, Token { tok: Tok::Ident(chars[start..i].iter().collect()), span })?;
            continue;
        }
        if c == '"' {
            let start = i;
            i += 1;
            let mut s = String::new();
            while i < chars.len() && chars[i] != '"' {
                if chars[i] == '\\' || chars[i] == '\n' {
                    return Err(Diag::new("LEX_BAD_STRING", "문자열 안 escape/줄바꿈은 spike에서 지원하지 않음", span));
                }
                s.push(chars[i]);
                i += 1;
            }
            if i >= chars.len() {
                return Err(Diag::new("LEX_BAD_STRING", "닫히지 않은 문자열", span));
            }
            i += 1;
            col += (i - start) as u32;
            push(&mut out, Token { tok: Tok::Str(s), span })?;
            continue;
        }
        let rest: String = chars[i..chars.len().min(i + 2)].iter().collect();
        match SYMS.iter().find(|s| rest.starts_with(**s)) {
            Some(s) => {
                i += s.len();
                col += s.len() as u32;
                push(&mut out, Token { tok: Tok::Sym(s), span })?;
            }
            None => return Err(Diag::new("LEX_UNEXPECTED_CHAR", format!("예상하지 못한 문자 `{c}`"), span)),
        }
    }
    out.push(Token { tok: Tok::Eof, span: Span { line, col } });
    Ok(out)
}

fn push(out: &mut Vec<Token>, token: Token) -> Result<(), Diag> {
    if out.len() >= MAX_TOKENS {
        return Err(token_limit(token.span));
    }
    out.push(token);
    Ok(())
}
