//! E 형식: 호스트 파일을 실행하지 않고, 공식 import로 바인딩된 `aip` 선언 블록의 정적 리터럴만 읽는다.
use crate::diag::{Diag, Span};
use crate::host::{is_ident, is_punct, official_import, span_at, statement_ends, tokenize, K, T};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Host {
    Ts,
    Py,
}

pub struct Block {
    pub text: String,
    pub line_base: u32,
    pub col_base: u32,
}

pub const TS_MODULE: &str = "@aip/define";

/// 공식 import 토큰 범위. TS는 모듈 문자열까지 확인한다.
pub fn imports(src: &str, toks: &[T], host: Host, name: &str) -> Vec<(usize, usize)> {
    official_import(toks, host, name)
        .into_iter()
        .filter(|(_, e)| match (&toks[*e].k, host) {
            (K::Str { body, .. }, Host::Ts) => &src[body.0..body.1] == TS_MODULE,
            (_, Host::Py) => true,
            _ => false,
        })
        .collect()
}

/// 선언 위치: TS `export default X` / `export const NAME = X`, Python 줄 시작 `NAME = X`.
pub fn decl_position(toks: &[T], i: usize, host: Host) -> bool {
    let p = |k: usize| i.checked_sub(k).map(|j| &toks[j]);
    match host {
        Host::Ts => {
            let default = matches!((p(2), p(1)), (Some(a), Some(b)) if is_ident(a, "export") && is_ident(b, "default"));
            let named = matches!((p(4), p(3), p(2), p(1)), (Some(a), Some(b), Some(c), Some(d))
                if is_ident(a, "export") && is_ident(b, "const") && matches!(c.k, K::Ident(_)) && is_punct(d, '='));
            default || named
        }
        Host::Py => matches!((p(2), p(1)), (Some(a), Some(b)) if a.nl && matches!(a.k, K::Ident(_)) && is_punct(b, '=')),
    }
}

/// `name` 식별자의 모든 등장을 분류한다. 공식 import 안·멤버 이름이 아닌 등장은 선언 위치여야 한다.
pub fn uses(src: &str, toks: &[T], host: Host, name: &str) -> Result<Vec<usize>, Diag> {
    let imp = imports(src, toks, host, name);
    let in_import = |i: usize| imp.iter().any(|(s, e)| i >= *s && i <= *e);
    let mut v = vec![];
    for (i, t) in toks.iter().enumerate() {
        if !is_ident(t, name) || in_import(i) || (i > 0 && is_punct(&toks[i - 1], '.')) {
            continue;
        }
        let stmt_start = (0..=i).rev().find(|&j| toks[j].nl || (j > 0 && is_punct(&toks[j - 1], ';'))).unwrap_or(0);
        if is_ident(&toks[stmt_start], "import") || is_ident(&toks[stmt_start], "from") {
            return Err(Diag::new("WRONG_BINDING", format!("`{name}`는 공식 모듈에서만 import"), span_at(src, t.start)));
        }
        if !decl_position(toks, i, host) {
            return Err(Diag::new("NON_LITERAL", format!("`{name}`는 공식 import와 export 선언 위치에서만 사용"), span_at(src, t.start)));
        }
        v.push(i);
    }
    if !v.is_empty() && imp.len() != 1 {
        let what = match host {
            Host::Ts => format!("import {{ {name} }} from \"{TS_MODULE}\""),
            Host::Py => format!("from aip.define import {name}"),
        };
        return Err(Diag::new("WRONG_BINDING", format!("`{what}`가 정확히 한 번 있어야 함"), Span::default()));
    }
    Ok(v)
}

pub fn extract(src: &str, host: Host) -> Result<Block, Diag> {
    let toks = tokenize(src, host)?;
    let mut blocks = vec![];
    for i in uses(src, &toks, host, "aip")? {
        let at = span_at(src, toks[i].start);
        let (body, next) = match host {
            Host::Ts => match toks.get(i + 1).map(|t| &t.k) {
                Some(K::Template { body, interp, escape }) => {
                    if *interp {
                        return Err(Diag::new("DYNAMIC_INTERPOLATION", "AIP 블록 안 `${...}` 보간 금지", at));
                    }
                    if *escape {
                        return Err(Diag::new("ESCAPE_UNSUPPORTED", "AIP 블록 안 escape는 cooked/raw 차이를 만들어 금지", at));
                    }
                    (*body, i + 2)
                }
                _ => return Err(Diag::new("NON_LITERAL", "`aip`는 tagged template(aip`...`)로만 사용", at)),
            },
            Host::Py => {
                let ok_shape = toks.get(i + 1).is_some_and(|t| is_punct(t, '(')) && toks.get(i + 3).is_some_and(|t| is_punct(t, ')'));
                match (ok_shape, toks.get(i + 2).map(|t| &t.k)) {
                    (true, Some(K::Str { prefix, triple, body, escape })) => {
                        if prefix.to_ascii_lowercase().contains('f') {
                            return Err(Diag::new("DYNAMIC_INTERPOLATION", "f-string 금지", at));
                        }
                        if !prefix.is_empty() || !triple {
                            return Err(Diag::new("NON_LITERAL", "접두사 없는 삼중 따옴표 문자열 하나만 허용", at));
                        }
                        if *escape {
                            return Err(Diag::new("ESCAPE_UNSUPPORTED", "AIP 블록 안 escape 금지", at));
                        }
                        (*body, i + 4)
                    }
                    (_, Some(K::Str { prefix, .. })) if prefix.to_ascii_lowercase().contains('f') => {
                        return Err(Diag::new("DYNAMIC_INTERPOLATION", "f-string 금지", at))
                    }
                    _ => return Err(Diag::new("NON_LITERAL", "aip(\"\"\"...\"\"\") 리터럴 하나만 허용(결합·format 금지)", at)),
                }
            }
        };
        if !statement_ends(&toks, next, host) {
            let sp = toks.get(next).map(|t| span_at(src, t.start)).unwrap_or(at);
            return Err(Diag::new("NON_LITERAL", "AIP 블록 뒤 결합·호출·가공 금지", sp));
        }
        let s = span_at(src, body.0);
        blocks.push(Block { text: src[body.0..body.1].to_string(), line_base: s.line, col_base: s.col });
    }
    match blocks.len() {
        1 => Ok(blocks.pop().unwrap()),
        0 => Err(Diag::new("NO_BLOCK", "AIP 블록 없음", Span::default())),
        _ => Err(Diag::new("MULTIPLE_BLOCKS", "spike는 파일당 블록 하나만 지원", Span::default())),
    }
}
