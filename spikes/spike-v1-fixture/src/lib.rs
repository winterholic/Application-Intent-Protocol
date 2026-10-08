pub mod ast;
pub mod diag;
pub mod extract;
pub mod hlit;
pub mod hmap;
pub mod host;
pub mod lexer;
mod limits;
pub mod parser;
pub mod sema;
mod string_literal;

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

pub fn load_str(src: &str, form: Form) -> Result<sema::Output, Vec<Diag>> {
    limits::check_source(src).map_err(|d| vec![d])?;
    let spec = match form {
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
    .map_err(|d| vec![d])?;
    sema::analyze(&spec)
}

pub fn digest(v: &Value) -> String {
    // serde_json 기본 Map은 BTreeMap이라 키 순서가 정규화된다.
    let s = serde_json::to_string(v).unwrap();
    Sha256::digest(s.as_bytes()).iter().map(|b| format!("{b:02x}")).collect()
}
