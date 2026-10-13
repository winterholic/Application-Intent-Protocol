pub mod ast;
pub mod codes;
pub mod diag;
mod enrich;
pub mod extract;
pub mod hlit;
pub mod hmap;
pub mod host;
pub mod isolation;
pub mod lexer;
mod limits;
pub mod parser;
pub mod sema;
mod string_literal;
pub mod syntax_card;

use diag::Diag;
use extract::Host;
use serde_json::Value;
use sha2::{Digest, Sha256};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Form {
    A,
    ETs,
    EPy,
    HTs,
    HPy,
}

pub fn form_of(path: &str) -> Option<Form> {
    [(".h.ts", Form::HTs), (".h.py", Form::HPy), (".e.ts", Form::ETs), (".e.py", Form::EPy), (".aip", Form::A)]
        .into_iter()
        .find(|(s, _)| path.ends_with(s))
        .map(|x| x.1)
}

/// A PoC program (`.aipoc`) given to the product checker. The PoC language (`entity`, `command`, `query`) is read by
/// `aip check`; reading it here would only report typos in a file that has none (founder decision 2026-10-13, audit F5).
pub fn wrong_language(path: &str) -> Option<Vec<Diag>> {
    path.ends_with(".aipoc").then(|| {
        vec![Diag::new(
            "DEF.WRONG_LANGUAGE",
            format!("`{path}` has the extension `.aipoc` of PoC programs, not of product definitions"),
            diag::Span { line: 1, col: 1 },
        )
        .with_help(
            "`aip service` reads product definitions (resource, expose, transition) in `.aip` files; a PoC program (entity, command, query) is checked with `aip check`. \
             If this file is a product definition, rename it to `.aip`.",
        )]
    })
}

pub fn load_str(src: &str, form: Form) -> Result<sema::Output, Vec<Diag>> {
    limits::check_source(src).map_err(|d| enrich::enrich(vec![d], src, form))?;
    match form {
        Form::A => parser::parse_spec(src, 1, 1),
        Form::ETs | Form::EPy => {
            let host = if form == Form::ETs { Host::Ts } else { Host::Py };
            extract::extract(src, host).and_then(|b| parser::parse_spec(&b.text, b.line_base, b.col_base))
        }
        Form::HTs | Form::HPy => {
            let host = if form == Form::HTs { Host::Ts } else { Host::Py };
            hlit::extract_define(src, host).and_then(|(lit, sp)| hmap::to_spec(&lit, sp))
        }
    }
    .map_err(|d| vec![d])
    .and_then(|spec| sema::analyze(&spec))
    .map_err(|ds| enrich::enrich(ds, src, form))
}

pub fn digest(v: &Value) -> String {
    // serde_json 기본 Map은 BTreeMap이라 키 순서가 정규화된다.
    let s = serde_json::to_string(v).unwrap();
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
