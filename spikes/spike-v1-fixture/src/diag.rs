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
    /// The segments `scope` is made of, for a tool that must not split a sentence.
    scope_path: Vec<String>,
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

/// The anchor of a position: `line:<n>:col:<m>`, 1-based, the column counted in Unicode code points. Advisories use the
/// same text (`advisories[].anchor`), so a tool reads the place of an error and of a warning the same way.
pub fn anchor(span: Span) -> String {
    format!("line:{}:col:{}", span.line, span.col)
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

    pub fn scope_path(&self) -> &[String] {
        self.notes.as_deref().map_or(&[], |n| n.scope_path.as_slice())
    }

    /// Whether the diagnostic points into the text. A few come from the whole definition (a missing `actor`, a file
    /// without a block, a source over the size limit); they carry the coordinate 0:0.
    pub fn located(&self) -> bool {
        self.span.line >= 1
    }

    pub(crate) fn set_scope_path(&mut self, path: Vec<String>) {
        if !path.is_empty() {
            let notes = self.notes.get_or_insert_default();
            notes.scope = Some(path.join(" > "));
            notes.scope_path = path;
        }
    }

    pub(crate) fn set_help(&mut self, help: Option<String>) {
        if help.is_some() || self.notes.is_some() {
            self.notes.get_or_insert_default().help = help;
        }
    }

    /// The wire form of the service output.
    ///
    /// Fields a tool may branch on: `code`, `severity`, `line`, `column`, and when present `located`, `anchor`,
    /// `scopePath`. Fields for people, whose wording may change: `message`, `help`, `scope`, `explain`. The first five
    /// are the historical ones and keep their meaning (a diagnostic without a position is still 0:0); every other field
    /// is additive and optional for a reader. Definition diagnostics are always errors.
    pub fn json(&self) -> Value {
        let mut m = Map::new();
        m.insert("code".into(), json!(self.code));
        m.insert("severity".into(), json!("error"));
        m.insert("message".into(), json!(self.msg));
        m.insert("line".into(), json!(self.span.line));
        m.insert("column".into(), json!(self.span.col));
        m.insert("located".into(), json!(self.located()));
        if self.located() {
            m.insert("anchor".into(), json!(anchor(self.span)));
        }
        if let Some(scope) = self.scope() {
            m.insert("scope".into(), json!(scope));
            m.insert("scopePath".into(), json!(self.scope_path()));
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
