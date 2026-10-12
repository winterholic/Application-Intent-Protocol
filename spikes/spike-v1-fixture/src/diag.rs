use serde_json::{json, Map, Value};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Span {
    pub line: u32,
    pub col: u32,
}

/// What `enrich` adds after the whole definition was read. Boxed on purpose: `Diag` is the error of
/// every recursive-descent function, so its size is part of the stack the parser needs at the nesting
/// limit (`tests/production_limits.rs` runs that limit on a small stack).
#[derive(Debug, Clone, Default)]
struct Notes {
    scope: Option<String>,
    help: Option<String>,
}

/// A definition error. `msg` says what is wrong at `span`; the scope (the declaration the position is
/// in, such as `resource Recruitment > expose read`) and the help (what to change) are added by
/// `enrich`, so the emitting sites stay short.
#[derive(Debug, Clone)]
pub struct Diag {
    pub code: &'static str,
    pub msg: String,
    pub span: Span,
    notes: Option<Box<Notes>>,
}

impl Diag {
    pub fn new(code: &'static str, msg: impl Into<String>, span: Span) -> Self {
        Diag { code, msg: msg.into(), span, notes: None }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.notes.get_or_insert_default().help = Some(help.into());
        self
    }

    pub fn scope(&self) -> Option<&str> {
        self.notes.as_deref().and_then(|n| n.scope.as_deref())
    }

    pub fn help(&self) -> Option<&str> {
        self.notes.as_deref().and_then(|n| n.help.as_deref())
    }

    pub(crate) fn set_scope(&mut self, scope: Option<String>) {
        if scope.is_some() {
            self.notes.get_or_insert_default().scope = scope;
        }
    }

    pub(crate) fn set_help(&mut self, help: Option<String>) {
        if help.is_some() || self.notes.is_some() {
            self.notes.get_or_insert_default().help = help;
        }
    }

    /// The wire form of the service output. The first four fields are the historical ones; `scope`,
    /// `help` and `explain` are additive.
    pub fn json(&self) -> Value {
        let mut m = Map::new();
        m.insert("code".into(), json!(self.code));
        m.insert("message".into(), json!(self.msg));
        m.insert("line".into(), json!(self.span.line));
        m.insert("column".into(), json!(self.span.col));
        if let Some(scope) = self.scope() {
            m.insert("scope".into(), json!(scope));
        }
        if let Some(help) = self.help() {
            m.insert("help".into(), json!(help));
        }
        if crate::codes::lookup(self.code).is_some() {
            m.insert("explain".into(), json!(format!("aip explain-code {}", self.code)));
        }
        Value::Object(m)
    }

    /// Text for a terminal: `file:line:col CODE message`, then the scope and the help.
    pub fn render(&self, file: &str) -> String {
        let mut out = format!("{file}:{}:{} {} {}", self.span.line, self.span.col, self.code, self.msg);
        if let Some(scope) = self.scope() {
            out.push_str(&format!("\n  in: {scope}"));
        }
        if let Some(help) = self.help() {
            out.push_str(&format!("\n  help: {help}"));
        }
        out
    }
}

impl fmt::Display for Diag {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{} {} {}", self.span.line, self.span.col, self.code, self.msg)
    }
}
