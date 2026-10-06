use aip_ir::codes;
use serde::Serialize;

/// Structured failure returned to clients. `code` is the stable taxonomy
/// key; `reason` is intent-specific (a `require ... else CODE`, a constraint).
#[derive(Debug, Clone, Serialize)]
pub struct AipError {
    pub code: String,
    pub intent: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    pub message: String,
    pub retryable: bool,
    /// Commit the transaction before reporting this error (spent attempts, audit).
    #[serde(skip)]
    pub commit: bool,
}

impl AipError {
    pub fn new(code: &str, intent: &str, message: impl Into<String>) -> Self {
        AipError {
            code: code.into(),
            intent: intent.into(),
            reason: None,
            path: None,
            message: message.into(),
            retryable: codes::retryable(code),
            commit: false,
        }
    }

    pub fn reason(mut self, r: impl Into<String>) -> Self {
        self.reason = Some(r.into());
        self
    }

    pub fn path(mut self, p: impl Into<String>) -> Self {
        self.path = Some(p.into());
        self
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn status(&self) -> u16 {
        codes::http_status(&self.code)
    }
}

impl std::fmt::Display for AipError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.code)?;
        if let Some(r) = &self.reason {
            write!(f, "({r})")?;
        }
        write!(f, ": {}", self.message)
    }
}

impl std::error::Error for AipError {}
