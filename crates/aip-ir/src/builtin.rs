//! Meaning of the extensions the language ships with: what a source's
//! signature looks like when the program does not say. Backends that verify a
//! webhook and the operator contract that tells a provider how to sign read
//! the same defaults from here.

use crate::facts::{callee_name, lit_text};
use crate::*;

/// How a webhook source signs its requests and where the event sits in the body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WebhookSettings {
    /// Extension call that names the source (`payments.stripe.webhook`).
    pub source: String,
    /// `stripe` (`t=..,v1=..` over "{t}.{body}") or `hmac-sha256` (plain HMAC of the body).
    pub scheme: String,
    pub header: String,
    /// Name of the environment variable that holds the shared secret; empty if none is configured.
    pub secret_env: String,
    pub event_path: String,
    pub id_path: String,
    pub payload_path: String,
}

/// Defaults of a source overridden by the options the program passes (`header:`, `secret:`, `event:`, `id:`, `payload:`).
pub fn webhook_settings(w: &Webhook) -> WebhookSettings {
    let opt = |k: &str| {
        w.via.args.iter().find_map(|a| match (&a.name, lit_text(&a.value)) {
            (Some(n), Some(v)) if n == k => Some(v.to_string()),
            _ => None,
        })
    };
    let source = callee_name(&w.via).to_string();
    // Stripe puts the event object under data.object
    let (scheme, header, secret, payload) = match source.as_str() {
        "payments.stripe.webhook" => ("stripe", "Stripe-Signature", "STRIPE_WEBHOOK_SECRET", "data.object"),
        _ => ("hmac-sha256", "X-Signature", "", "data"),
    };
    WebhookSettings {
        source,
        scheme: scheme.into(),
        header: opt("header").unwrap_or_else(|| header.into()),
        secret_env: opt("secret").unwrap_or_else(|| secret.into()),
        event_path: opt("event").unwrap_or_else(|| "type".into()),
        id_path: opt("id").unwrap_or_else(|| "id".into()),
        payload_path: opt("payload").unwrap_or_else(|| payload.into()),
    }
}

/// Values of the built-in enums that forms bring with them.
pub fn form_enums(core: &Program) -> BTreeMap<String, Vec<String>> {
    let words = |ws: &[&str]| ws.iter().map(|w| w.to_string()).collect::<Vec<_>>();
    let mut out = BTreeMap::new();
    if core.forms.iter().any(|f| matches!(f, Form::Approval(_))) {
        out.insert("ApprovalStatus".to_string(), words(&["NONE", "PENDING", "APPROVED", "REJECTED", "CANCELLED", "EXPIRED"]));
        out.insert("ApprovalDecision".to_string(), words(&["APPROVE", "REJECT"]));
    }
    if core.forms.iter().any(|f| matches!(f, Form::Consent(_))) {
        out.insert("ConsentStatus".to_string(), words(&["NONE", "GIVEN", "OUTDATED", "WITHDRAWN"]));
    }
    if core.forms.iter().any(|f| matches!(f, Form::Job(_))) {
        out.insert("JobStatus".to_string(), words(&["QUEUED", "RUNNING", "DONE", "FAILED"]));
    }
    out
}

/// Languages a `search` can name. A backend maps each to the text analysis it has; `korean` is the one that needs a
/// morphological analyzer the common engines do not ship, so it is searched by word prefix instead (see [`search_is_approximate`]).
pub const SEARCH_LANGUAGES: &[&str] = &[
    "english",
    "korean",
    "german",
    "french",
    "spanish",
    "italian",
    "portuguese",
    "dutch",
    "russian",
    "swedish",
    "norwegian",
    "danish",
    "finnish",
    "hungarian",
    "romanian",
    "turkish",
];

/// Weights of a search field, highest first.
pub const SEARCH_WEIGHTS: &[&str] = &["A", "B", "C", "D"];

/// Does a search of this language match by word prefix instead of by word stem?
pub fn search_is_approximate(language: Option<&str>) -> bool {
    language == Some("korean")
}
