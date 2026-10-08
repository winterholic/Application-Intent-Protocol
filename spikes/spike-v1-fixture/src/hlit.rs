//! H 형식: 호스트 언어의 `define({...})` 인자를 실행하지 않고 리터럴 부분집합으로 읽는다.
use crate::diag::{Diag, Span};
use crate::extract::{uses, Host};
use crate::host::{is_punct, normalized_source, statement_ends, tokenize};
use crate::limits::MAX_LITERAL_DEPTH;
use crate::string_literal::{decode_body, Flavor};

#[derive(Debug, Clone)]
pub enum Lit {
    Str(String),
    Int(i64),
    Bool(bool),
    Null,
    Arr(Vec<(Lit, Span)>),
    /// 키 순서를 보존한다. predicate 매개변수 순서가 키 순서에 의존하기 때문이다.
    Obj(Vec<(String, Lit, Span)>),
}

struct P<'a> {
    s: &'a [u8],
    src: &'a str,
    i: usize,
    host: Host,
    value_depth: usize,
}

type R<T> = Result<T, Diag>;

impl<'a> P<'a> {
    fn sp(&self) -> Span {
        let line = self.src[..self.i].matches('\n').count() as u32 + 1;
        let ls = self.src[..self.i].rfind('\n').map(|k| k + 1).unwrap_or(0);
        let col = self.src[ls..self.i].chars().count() as u32 + 1;
        Span { line, col }
    }
    fn err<T>(&self, code: &'static str, msg: impl Into<String>) -> R<T> {
        Err(Diag::new(code, msg, self.sp()))
    }
    fn ws(&mut self) {
        loop {
            while self.i < self.s.len() && (self.s[self.i] as char).is_whitespace() {
                self.i += 1;
            }
            let c = match self.host {
                Host::Ts => self.src[self.i..].starts_with("//"),
                Host::Py => self.src[self.i..].starts_with('#'),
            };
            if c {
                while self.i < self.s.len() && self.s[self.i] != b'\n' {
                    self.i += 1;
                }
            } else if self.host == Host::Ts && self.src[self.i..].starts_with("/*") {
                match self.src[self.i + 2..].find("*/") {
                    Some(k) => self.i += k + 4,
                    None => self.i = self.s.len(),
                }
            } else {
                return;
            }
        }
    }
    fn peek(&self) -> u8 {
        *self.s.get(self.i).unwrap_or(&0)
    }
    fn eat(&mut self, c: u8) -> bool {
        self.ws();
        if self.peek() == c {
            self.i += 1;
            true
        } else {
            false
        }
    }
    fn word(&mut self) -> String {
        let st = self.i;
        while self.i < self.s.len() && (self.s[self.i].is_ascii_alphanumeric() || self.s[self.i] == b'_' || self.s[self.i] == b'$') {
            self.i += 1;
        }
        self.src[st..self.i].to_string()
    }
    fn string(&mut self) -> R<String> {
        let q = self.peek();
        self.i += 1;
        let st = self.i;
        while self.i < self.s.len() && self.s[self.i] != q {
            if self.s[self.i] == b'\\' {
                self.i += 1;
                if self.i >= self.s.len() {
                    return self.err("NON_LITERAL", "끝나지 않은 문자열 escape");
                }
                let next = self.src[self.i..].chars().next().unwrap();
                self.i += next.len_utf8();
                continue;
            }
            let next = self.src[self.i..].chars().next().unwrap();
            self.i += next.len_utf8();
        }
        if self.i >= self.s.len() {
            return self.err("NON_LITERAL", "닫히지 않은 문자열");
        }
        let v = decode_body(&self.src[st..self.i], Flavor::Host, false).map_err(|reason| Diag::new("NON_LITERAL", reason, self.sp()))?;
        self.i += 1;
        Ok(v)
    }
    fn value(&mut self) -> R<Lit> {
        if self.value_depth >= MAX_LITERAL_DEPTH {
            return self.err("H_LITERAL_NESTING", "정의 리터럴 중첩 hard ceiling을 벗어남");
        }
        self.value_depth += 1;
        let result = self.literal_value();
        self.value_depth -= 1;
        result
    }

    fn literal_value(&mut self) -> R<Lit> {
        self.ws();
        match self.peek() {
            b'{' => self.object(),
            b'[' => {
                self.i += 1;
                let mut v = vec![];
                loop {
                    if self.eat(b']') {
                        return Ok(Lit::Arr(v));
                    }
                    self.ws();
                    let sp = self.sp();
                    v.push((self.value()?, sp));
                    if !self.eat(b',') {
                        if self.eat(b']') {
                            return Ok(Lit::Arr(v));
                        }
                        return self.err("NON_LITERAL", "배열에 `,` 또는 `]` 필요");
                    }
                }
            }
            b'"' | b'\'' => Ok(Lit::Str(self.string()?)),
            b'`' => self.err("NON_LITERAL", "template literal 금지(보간 가능)"),
            b'.' => self.err("NON_LITERAL", "spread 금지"),
            c if c.is_ascii_digit() || c == b'-' => {
                let st = self.i;
                self.i += 1;
                while self.peek().is_ascii_digit() {
                    self.i += 1;
                }
                match self.src[st..self.i].parse() {
                    Ok(n) if !matches!(self.peek(), b'.' | b'e' | b'E' | b'_') => Ok(Lit::Int(n)),
                    _ => self.err("NON_LITERAL", "정수 리터럴만 허용"),
                }
            }
            c if c.is_ascii_alphabetic() || c == b'_' || c == b'$' => {
                let w = self.word();
                self.ws();
                if (self.peek() == b'"' || self.peek() == b'\'') && self.host == Host::Py {
                    return self.err("DYNAMIC_INTERPOLATION", format!("문자열 접두사 `{w}` 금지(f-string 등)"));
                }
                match (self.host, w.as_str()) {
                    (Host::Ts, "true") | (Host::Py, "True") => Ok(Lit::Bool(true)),
                    (Host::Ts, "false") | (Host::Py, "False") => Ok(Lit::Bool(false)),
                    (Host::Ts, "null") | (Host::Py, "None") => Ok(Lit::Null),
                    _ => self.err("NON_LITERAL", format!("변수·호출·함수 `{w}` 금지. 정의는 리터럴만")),
                }
            }
            _ => self.err("NON_LITERAL", "리터럴 필요"),
        }
    }
    fn object(&mut self) -> R<Lit> {
        self.i += 1;
        let mut v: Vec<(String, Lit, Span)> = vec![];
        loop {
            if self.eat(b'}') {
                return Ok(Lit::Obj(v));
            }
            self.ws();
            let sp = self.sp();
            let key = match self.peek() {
                b'"' | b'\'' => self.string()?,
                b'[' => return self.err("NON_LITERAL", "계산된 키 금지"),
                b'.' => return self.err("NON_LITERAL", "spread 금지"),
                c if c.is_ascii_alphabetic() || c == b'_' => {
                    if self.host == Host::Py {
                        return self.err("NON_LITERAL", "Python dict 키는 문자열 리터럴");
                    }
                    self.word()
                }
                _ => return self.err("NON_LITERAL", "키 필요"),
            };
            if v.iter().any(|x| x.0 == key) {
                return Err(Diag::new("DUPLICATE", format!("키 `{key}` 중복"), sp));
            }
            if !self.eat(b':') {
                return self.err("NON_LITERAL", "`:` 필요(축약 속성·메서드 금지)");
            }
            self.ws();
            // 값의 위치를 기록한다. 정책 문자열 안 오류의 열을 원본 기준으로 맞추기 위해서다.
            let _key_span = sp;
            let vsp = self.sp();
            let val = self.value()?;
            v.push((key, val, vsp));
            if !self.eat(b',') {
                if self.eat(b'}') {
                    return Ok(Lit::Obj(v));
                }
                return self.err("NON_LITERAL", "객체에 `,` 또는 `}` 필요");
            }
        }
    }
}

pub fn extract_define(src: &str, host: Host) -> R<(Lit, Span)> {
    let normalized = normalized_source(src);
    let src = normalized.as_ref();
    let toks = tokenize(src, host)?;
    let found = uses(src, &toks, host, "define")?;
    let i = match found.len() {
        1 => found[0],
        0 => return Err(Diag::new("NO_BLOCK", "define(...) 선언 없음", Span::default())),
        _ => return Err(Diag::new("MULTIPLE_BLOCKS", "spike는 파일당 define 하나만 지원", Span::default())),
    };
    if !toks.get(i + 1).is_some_and(|t| is_punct(t, '(')) {
        return Err(Diag::new("NON_LITERAL", "`define`은 직접 호출로만 사용", crate::host::span_at(src, toks[i].start)));
    }
    let mut p = P { s: src.as_bytes(), src, i: toks[i + 1].end, host, value_depth: 0 };
    p.ws();
    if p.peek() != b'{' {
        return p.err("NON_LITERAL", "define 인자는 객체 리터럴 하나");
    }
    let sp = p.sp();
    let v = p.object()?;
    p.eat(b',');
    if !p.eat(b')') {
        return p.err("NON_LITERAL", "define 인자는 하나");
    }
    let next = toks.iter().position(|t| t.start >= p.i).unwrap_or(toks.len());
    if !statement_ends(&toks, next, host) {
        return Err(Diag::new("NON_LITERAL", "define(...) 뒤 결합·호출·가공 금지", crate::host::span_at(src, toks[next].start)));
    }
    Ok((v, sp))
}
