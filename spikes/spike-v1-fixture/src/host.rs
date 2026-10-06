//! TS/Python 소스를 실행하지 않고 토큰으로만 나눈다. 주석과 문자열 안의 `aip`/`define`을 선언으로 오인하지 않기 위해서다.
//! 한계: TS 정규식 리터럴은 인식하지 않는다. 정규식 안 따옴표가 있으면 경계 판정이 틀릴 수 있다.
use crate::diag::{Diag, Span};
use crate::extract::Host;
use crate::limits::{check_source, token_limit, MAX_TOKENS};

#[derive(Debug, Clone, PartialEq)]
pub enum K {
    Ident(String),
    Punct(char),
    /// 문자열. 접두사(Python), 삼중 따옴표 여부, 본문 byte 범위, escape 포함 여부.
    Str {
        prefix: String,
        triple: bool,
        body: (usize, usize),
        escape: bool,
    },
    /// TS template. `${` 보간·escape 포함 여부.
    Template {
        body: (usize, usize),
        interp: bool,
        escape: bool,
    },
    Num,
}

#[derive(Debug, Clone)]
pub struct T {
    pub k: K,
    pub start: usize,
    pub end: usize,
    /// 앞에 줄바꿈이 있었는지. 문장 경계 판정에 쓴다.
    pub nl: bool,
}

pub fn span_at(src: &str, idx: usize) -> Span {
    let line = src[..idx].matches('\n').count() as u32 + 1;
    let ls = src[..idx].rfind('\n').map(|i| i + 1).unwrap_or(0);
    Span { line, col: src[ls..idx].chars().count() as u32 + 1 }
}

fn is_id_start(c: u8) -> bool {
    c.is_ascii_alphabetic() || c == b'_' || c == b'$'
}
fn is_id(c: u8) -> bool {
    is_id_start(c) || c.is_ascii_digit()
}

pub fn tokenize(src: &str, host: Host) -> Result<Vec<T>, Diag> {
    check_source(src)?;
    let b = src.as_bytes();
    let mut out = vec![];
    let (mut i, mut nl) = (0usize, true);
    let err = |code, msg: &str, at: usize| Err(Diag::new(code, msg.to_string(), span_at(src, at)));
    while i < b.len() {
        let c = b[i];
        if c == b'\n' {
            nl = true;
            i += 1;
            continue;
        }
        if c.is_ascii_whitespace() {
            i += 1;
            continue;
        }
        let comment_line = match host {
            Host::Ts => b[i..].starts_with(b"//"),
            Host::Py => c == b'#',
        };
        if comment_line {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if host == Host::Ts && b[i..].starts_with(b"/*") {
            match src.get(i + 2..).and_then(|x| x.find("*/")) {
                Some(k) => {
                    nl |= b[i..i + 2 + k].contains(&b'\n');
                    i += k + 4;
                    continue;
                }
                None => return err("HOST_LEX", "닫히지 않은 블록 주석", i),
            }
        }
        let start = i;
        if out.len() >= MAX_TOKENS {
            return Err(token_limit(span_at(src, start)));
        }
        if is_id_start(c) {
            while i < b.len() && is_id(b[i]) {
                i += 1;
            }
            let word = &src[start..i];
            // Python 문자열 접두사(r, b, f, u 조합) 바로 뒤 따옴표면 문자열이다.
            if host == Host::Py && i < b.len() && (b[i] == b'"' || b[i] == b'\'') && word.len() <= 2 && word.chars().all(|ch| "rRbBuUfF".contains(ch))
            {
                let (t, e) = py_string(src, start, i)?;
                out.push(T { k: t, start, end: e, nl });
                nl = false;
                i = e;
                continue;
            }
            out.push(T { k: K::Ident(word.to_string()), start, end: i, nl });
            nl = false;
            continue;
        }
        if c.is_ascii_digit() {
            while i < b.len() && (b[i].is_ascii_alphanumeric() || b[i] == b'.' || b[i] == b'_') {
                i += 1;
            }
            out.push(T { k: K::Num, start, end: i, nl });
            nl = false;
            continue;
        }
        if c == b'"' || c == b'\'' {
            let (t, e) = match host {
                Host::Py => py_string(src, start, i)?,
                Host::Ts => {
                    let mut j = i + 1;
                    let mut escape = false;
                    while j < b.len() && b[j] != c {
                        if b[j] == b'\\' {
                            escape = true;
                            j += 1;
                        } else if b[j] == b'\n' {
                            return err("HOST_LEX", "닫히지 않은 문자열", start);
                        }
                        j += 1;
                    }
                    if j >= b.len() {
                        return err("HOST_LEX", "닫히지 않은 문자열", start);
                    }
                    (K::Str { prefix: String::new(), triple: false, body: (i + 1, j), escape }, j + 1)
                }
            };
            out.push(T { k: t, start, end: e, nl });
            nl = false;
            i = e;
            continue;
        }
        if c == b'`' && host == Host::Ts {
            let mut j = i + 1;
            let (mut interp, mut escape) = (false, false);
            while j < b.len() && b[j] != b'`' {
                if b[j] == b'\\' {
                    escape = true;
                    j += 2;
                    continue;
                }
                if b[j..].starts_with(b"${") {
                    interp = true;
                    let mut depth = 0i32;
                    while j < b.len() {
                        match b[j] {
                            b'{' => depth += 1,
                            b'}' => {
                                depth -= 1;
                                if depth == 0 {
                                    break;
                                }
                            }
                            _ => {}
                        }
                        j += 1;
                    }
                }
                j += 1;
            }
            if j >= b.len() {
                return err("HOST_LEX", "닫히지 않은 template", start);
            }
            out.push(T { k: K::Template { body: (i + 1, j), interp, escape }, start, end: j + 1, nl });
            nl = false;
            i = j + 1;
            continue;
        }
        // 문자열·주석 밖 비ASCII 문자는 글자 단위로 한 토큰이 된다(byte 중간 위치를 만들지 않음).
        let ch = src[i..].chars().next().unwrap();
        out.push(T { k: K::Punct(ch), start, end: i + ch.len_utf8(), nl });
        nl = false;
        i += ch.len_utf8();
    }
    Ok(out)
}

fn py_string(src: &str, start: usize, q_at: usize) -> Result<(K, usize), Diag> {
    let b = src.as_bytes();
    let prefix = src[start..q_at].to_string();
    let q = b[q_at];
    let triple = b[q_at..].starts_with(if q == b'"' { b"\"\"\"" } else { b"'''" });
    let qlen = if triple { 3 } else { 1 };
    let close: &str = match (triple, q) {
        (true, b'"') => "\"\"\"",
        (true, _) => "'''",
        (false, b'"') => "\"",
        _ => "'",
    };
    let mut j = q_at + qlen;
    let mut escape = false;
    while j < b.len() {
        if b[j] == b'\\' {
            escape = true;
            // raw 문자열도 `\'`는 문자열을 닫지 않는다(백슬래시가 내용에 남을 뿐). 경계 판정은 항상 두 글자를 건너뛴다(R2-01).
            j += 2;
            continue;
        }
        if !triple && b[j] == b'\n' {
            break;
        }
        if b[j..].starts_with(close.as_bytes()) {
            return Ok((K::Str { prefix, triple, body: (q_at + qlen, j), escape }, j + qlen));
        }
        j += 1;
    }
    Err(Diag::new("HOST_LEX", "닫히지 않은 문자열", span_at(src, start)))
}

pub fn is_ident(t: &T, s: &str) -> bool {
    matches!(&t.k, K::Ident(x) if x == s)
}
pub fn is_punct(t: &T, c: char) -> bool {
    t.k == K::Punct(c)
}

/// TS: 공식 import 줄 `import { name } from "@aip/define"`의 토큰 범위.
/// Python: `from aip.define import name`.
pub fn official_import(toks: &[T], host: Host, name: &str) -> Vec<(usize, usize)> {
    let mut v = vec![];
    for i in 0..toks.len() {
        match host {
            Host::Ts => {
                if i + 5 < toks.len()
                    && is_ident(&toks[i], "import")
                    && is_punct(&toks[i + 1], '{')
                    && is_ident(&toks[i + 2], name)
                    && is_punct(&toks[i + 3], '}')
                    && is_ident(&toks[i + 4], "from")
                    && matches!(&toks[i + 5].k, K::Str { .. })
                {
                    v.push((i, i + 5));
                }
            }
            Host::Py => {
                if i + 5 < toks.len()
                    && toks[i].nl
                    && is_ident(&toks[i], "from")
                    && is_ident(&toks[i + 1], "aip")
                    && is_punct(&toks[i + 2], '.')
                    && is_ident(&toks[i + 3], "define")
                    && is_ident(&toks[i + 4], "import")
                    && is_ident(&toks[i + 5], name)
                    && toks.get(i + 6).is_none_or(|t| t.nl)
                {
                    v.push((i, i + 5));
                }
            }
        }
    }
    v
}

const TS_STMT: [&str; 10] = ["export", "import", "const", "let", "var", "function", "class", "type", "interface", "async"];

/// 선언 표현식 뒤에 이어 붙는 가공(결합·호출·멤버 접근·연산)이 없는지 본다.
pub fn statement_ends(toks: &[T], next: usize, host: Host) -> bool {
    match toks.get(next) {
        None => true,
        Some(t) if is_punct(t, ';') => true,
        Some(t) if !t.nl => false,
        Some(t) => match host {
            // 줄이 바뀌어도 TS는 연산자로 시작하는 줄을 앞 식에 잇는다(ASI 없음). 문장 키워드만 허용한다.
            Host::Ts => matches!(&t.k, K::Ident(w) if TS_STMT.contains(&w.as_str())),
            // Python은 괄호 밖 줄바꿈이 문장 끝이다. 다음 줄이 `.`/연산자로 시작하면 문법 오류라 실행 경로가 아니다.
            Host::Py => true,
        },
    }
}

pub fn decl_tokens(src: &str, t: &T) -> String {
    src[t.start..t.end].to_string()
}
