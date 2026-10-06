use crate::diag::{Diag, Span};

pub(crate) const MAX_SOURCE_BYTES: usize = 1024 * 1024;
pub(crate) const MAX_TOKENS: usize = 65_536;
pub(crate) const MAX_EXPRESSION_DEPTH: usize = 64;
pub(crate) const MAX_AST_DEPTH: usize = 64;
pub(crate) const MAX_EXPR_NODES: usize = 4_096;
pub(crate) const MAX_LITERAL_DEPTH: usize = 64;

pub(crate) fn check_source(src: &str) -> Result<(), Diag> {
    if src.len() > MAX_SOURCE_BYTES {
        Err(Diag::new("SOURCE_TOO_LARGE", format!("정의가 {} byte hard ceiling을 넘음", MAX_SOURCE_BYTES), Span::default()))
    } else {
        Ok(())
    }
}

pub(crate) fn token_limit(span: Span) -> Diag {
    Diag::new("TOKEN_LIMIT", format!("정의가 {} token hard ceiling을 넘음", MAX_TOKENS), span)
}
