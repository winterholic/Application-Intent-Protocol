use serde::Serialize;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub line: u32,
    pub col: u32,
}

impl Span {
    pub fn to(self, other: Span) -> Span {
        Span { start: self.start, end: other.end.max(self.end), line: self.line, col: self.col }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Error,
    Warning,
}

/// A structured diagnostic. `code` is the stable key from `spec/diagnostics.md`;
/// `help` is written so that a human or an LLM can act on it without reading
/// the compiler source.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    pub severity: Severity,
    pub code: String,
    pub message: String,
    pub span: Span,
    pub help: Option<String>,
}

/// Same fields and order as before; a registered code additionally carries `explain`, the command
/// that prints its meaning and fix (`aip explain-code <code>`).
impl Serialize for Diagnostic {
    fn serialize<S: serde::Serializer>(&self, ser: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let explain = aip_ir::codes::lookup(&self.code).map(|_| format!("aip explain-code {}", self.code));
        let n = 4 + usize::from(self.help.is_some()) + usize::from(explain.is_some());
        let mut st = ser.serialize_struct("Diagnostic", n)?;
        st.serialize_field("severity", &self.severity)?;
        st.serialize_field("code", &self.code)?;
        st.serialize_field("message", &self.message)?;
        st.serialize_field("span", &self.span)?;
        if let Some(h) = &self.help {
            st.serialize_field("help", h)?;
        }
        if let Some(e) = &explain {
            st.serialize_field("explain", e)?;
        }
        st.end()
    }
}

impl Diagnostic {
    pub fn error(code: &str, message: impl Into<String>, span: Span) -> Self {
        Diagnostic { severity: Severity::Error, code: code.to_string(), message: message.into(), span, help: None }
    }

    pub fn warning(code: &str, message: impl Into<String>, span: Span) -> Self {
        Diagnostic { severity: Severity::Warning, code: code.to_string(), message: message.into(), span, help: None }
    }

    pub fn with_help(mut self, help: impl Into<String>) -> Self {
        self.help = Some(help.into());
        self
    }

    pub fn render(&self, file: &str) -> String {
        let sev = match self.severity {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
        let head = format!("{sev} {} {file}:{}:{}  {}", self.code, self.span.line, self.span.col, self.message);
        match &self.help {
            Some(h) => format!("{head}\n  help: {h}"),
            None => head,
        }
    }
}
