use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

#[derive(Debug, Clone)]
pub struct Diag {
    pub code: &'static str,
    pub msg: String,
    pub span: Span,
}

impl Diag {
    pub fn new(code: &'static str, msg: impl Into<String>, span: Span) -> Self {
        Diag { code, msg: msg.into(), span }
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{} {} {}", self.span.line, self.span.col, self.code, self.msg)
    }
}
