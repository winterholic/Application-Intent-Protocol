//! Login, signup and token refresh for deployments whose users authenticate with a password the service itself checks.
//! The transport owns what every route shares (path allow-list, body bound, JSON shape, CORS with credentials, `Set-Cookie`,
//! no caching); the adapter that implements [`CredentialAuth`] owns the credential store, the password check and the token
//! minting. Nothing in a request names the actor, role or owner of what it creates: a body that carries such a key is the
//! adapter's to refuse.
use serde_json::Value;
use spike_v2_read::backend::BackendFuture;

/// The only paths served here. Every one is a POST.
pub const ROUTES: [&str; 5] = ["/auth/check-email", "/auth/signup", "/auth/login", "/auth/reissue", "/auth/logout"];

/// These bodies are an email, a password, a name and a short list. Anything larger is not one of them.
pub const MAX_BODY: usize = 16 * 1024;

pub fn is_route(path: &str) -> bool {
    ROUTES.contains(&path)
}

pub struct AuthRequest<'a> {
    pub route: &'a str,
    /// A JSON object, or `{}` for a request without a body.
    pub body: Value,
    /// The raw `Cookie` header, for the adapter to read its own refresh cookie from.
    pub cookie: Option<&'a str>,
}

pub struct AuthReply {
    pub status: u16,
    pub body: Value,
    /// A complete `Set-Cookie` field value, built by the adapter that knows the cookie's lifetime and scope.
    pub set_cookie: Option<String>,
}

impl AuthReply {
    pub fn fail(status: u16, code: &str, reason: &str) -> Self {
        Self { status, body: serde_json::json!({"ok": false, "code": code, "reason": reason}), set_cookie: None }
    }
}

pub trait CredentialAuth: Send + Sync {
    fn handle<'a>(&'a self, request: AuthRequest<'a>) -> BackendFuture<'a, AuthReply>;
}

/// The status line for the statuses an adapter may answer with. Anything else is a server fault, never passed through.
pub(crate) fn status_line(status: u16) -> &'static str {
    match status {
        200 => "200 OK",
        201 => "201 Created",
        400 => "400 Bad Request",
        401 => "401 Unauthorized",
        403 => "403 Forbidden",
        409 => "409 Conflict",
        422 => "422 Unprocessable Entity",
        429 => "429 Too Many Requests",
        503 => "503 Service Unavailable",
        _ => "500 Internal Server Error",
    }
}
