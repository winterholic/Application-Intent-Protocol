//! Intents that L3 forms stand for. A form is not an intent in Core IR, but
//! clients call what it lowers to: `RequestX` / `VerifyX` for a verification,
//! `IssueX` / `RedeemX` for a grant link, four commands and a status query for
//! an approval, a start command and a status query for a job, `Give` / `Withdraw`
//! and a status query for a consent, `Start` / `Stop` for an impersonation,
//! `Publish` / `Discard` for a publishable entity. Names,
//! parameters and the facts of each intent are decided here once; backends add
//! the execution and the contract adds the output shapes.

use crate::facts::{self, Facts};
use crate::*;

/// One callable intent a form stands for.
#[derive(Debug, Clone)]
pub struct FormIntent {
    pub name: String,
    pub params: Vec<Param>,
    pub facts: Facts,
    pub kind: FormIntentKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FormIntentKind {
    Command,
    /// Answers about one row, so it never pages.
    StatusQuery,
}

fn text() -> Type {
    Type::Text { min: None, max: None, trim: false, lower: false, pattern: None }
}

fn param(name: &str, ty: Type, optional: bool) -> Param {
    Param { name: name.to_string(), ty, optional, default: None }
}

fn lower_first(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_lowercase().chain(c).collect()).unwrap_or_default()
}

/// A form-made command: never idempotent by key.
fn command(core: &Program, name: String, params: Vec<Param>, mut f: Facts) -> FormIntent {
    facts::finish_command(core, false, &mut f);
    facts::tenant_errors(core, &params, false, Some(&f.writes.clone()), &mut f);
    FormIntent { name, params, facts: f, kind: FormIntentKind::Command }
}

fn status(name: String, params: Vec<Param>, f: Facts) -> FormIntent {
    FormIntent { name, params, facts: f, kind: FormIntentKind::StatusQuery }
}

/// `IssueX(scope...)` and `RedeemX(token, redeem params...)`.
pub struct GrantLinkIntents {
    pub issue: FormIntent,
    pub redeem: FormIntent,
}

pub fn grant_link(core: &Program, g: &GrantLink) -> GrantLinkIntents {
    let mut f = Facts::default();
    facts::param_loads(&g.scope, &mut f);
    f.code(codes::AUTH_FORBIDDEN, None);
    let issue = command(core, format!("Issue{}", g.name), g.scope.clone(), f);

    let mut all = vec![param("token", text(), false)];
    all.extend(g.redeem_params.iter().cloned());
    let mut f = Facts::default();
    f.code(codes::PRECONDITION_FAILED, Some("INVALID_GRANT_LINK"));
    for r in &g.requires {
        f.code(codes::PRECONDITION_FAILED, Some(&r.code));
    }
    // `grants: rel(..) as ROLE` inserts the row the relation selects
    if let Some((rel_call, _)) = &g.grants
        && let Node::Call(call) = &rel_call.node
        && let Some(rel) = core.relations.get(facts::callee_name(call))
        && let Node::The { set } = &rel.body.node
        && let SetSource::Entity { entity } = &set.source
    {
        f.writes.insert(entity.clone());
    }
    if let Some(b) = &g.on_redeem {
        facts::collect_stmts(core, b, &mut f);
    }
    GrantLinkIntents { issue, redeem: command(core, format!("Redeem{}", g.name), all, f) }
}

/// `RequestX(target)` and `VerifyX(target, code)`.
pub struct VerificationIntents {
    pub request: FormIntent,
    pub verify: FormIntent,
}

pub fn verification(core: &Program, v: &Verification) -> VerificationIntents {
    let target = param("target", v.target.clone(), false);
    let mut f = Facts::default();
    if v.target_where.is_some() {
        f.code(codes::PRECONDITION_FAILED, Some("VERIFICATION_TARGET_INVALID"));
    }
    if v.resend_after_seconds.is_some() {
        f.code(codes::PRECONDITION_FAILED, Some("VERIFICATION_RESEND_TOO_SOON"));
    }
    f.effects.insert(facts::callee_name(&v.deliver).to_string());
    let request = command(core, format!("Request{}", v.name), vec![target.clone()], f);

    let mut f = Facts::default();
    f.code(codes::PRECONDITION_FAILED, Some("VERIFICATION_NOT_FOUND"));
    f.code(codes::PRECONDITION_FAILED, Some("VERIFICATION_CODE_MISMATCH"));
    facts::collect_stmts(core, &v.on_verified.2, &mut f);
    let verify = command(core, format!("Verify{}", v.name), vec![target, param("code", text(), false)], f);
    VerificationIntents { request, verify }
}

/// `RequestX`, `ApproveX`, `RejectX`, `CancelX` and the query `XStatus`.
pub struct ApprovalIntents {
    pub request: FormIntent,
    pub approve: FormIntent,
    pub reject: FormIntent,
    pub cancel: FormIntent,
    pub status: FormIntent,
}

pub fn approval(core: &Program, a: &Approval) -> ApprovalIntents {
    let subject = param(&lower_first(&a.entity), Type::Ref { entity: a.entity.clone() }, false);
    let not_found = || {
        let mut f = Facts::default();
        facts::param_loads(std::slice::from_ref(&subject), &mut f);
        f
    };
    let pending = |f: &mut Facts| {
        f.code(codes::PRECONDITION_FAILED, Some("APPROVAL_NOT_PENDING"));
        if a.expires_seconds.is_some() {
            f.code(codes::PRECONDITION_FAILED, Some("APPROVAL_EXPIRED"));
        }
    };

    let mut f = not_found();
    if a.requested_by.is_some() {
        f.code(codes::AUTH_FORBIDDEN, None);
    }
    f.code(codes::CONFLICT_UNIQUE, Some("APPROVAL_ALREADY_PENDING"));
    let request = command(core, format!("Request{}", a.name), vec![subject.clone()], f);

    let vote = |verb: &str, note: &str, block: &[Stmt]| {
        let mut f = not_found();
        pending(&mut f);
        f.code(codes::AUTH_FORBIDDEN, Some("NOT_AN_APPROVER"));
        if a.no_self {
            f.code(codes::AUTH_FORBIDDEN, Some("SELF_APPROVAL_FORBIDDEN"));
        }
        f.code(codes::CONFLICT_UNIQUE, Some("ALREADY_VOTED"));
        // a vote is the voter's own act, like consent
        refuse_impersonated(core, &mut f);
        facts::collect_stmts(core, block, &mut f);
        command(core, format!("{verb}{}", a.name), vec![subject.clone(), param(note, text(), true)], f)
    };
    let approve = vote("Approve", "comment", &a.on_approved);
    let reject = vote("Reject", "reason", &a.on_rejected);

    let mut f = not_found();
    pending(&mut f);
    f.code(codes::AUTH_FORBIDDEN, Some("ONLY_REQUESTER_CAN_CANCEL"));
    let cancel = command(core, format!("Cancel{}", a.name), vec![subject.clone()], f);

    let mut f = not_found();
    f.code(codes::INPUT_INVALID, None);
    let status = status(format!("{}Status", a.name), vec![subject], f);
    ApprovalIntents { request, approve, reject, cancel, status }
}

/// The start command `X(params...)` and the query `XStatus(job)`.
pub struct JobIntents {
    pub start: FormIntent,
    pub status: FormIntent,
}

pub fn job(core: &Program, j: &Job) -> JobIntents {
    let mut f = Facts::default();
    facts::param_loads(&j.params, &mut f);
    f.code(codes::AUTH_FORBIDDEN, j.allow.code.as_deref());
    facts::consent_errors(core, &j.name, &mut f);
    let start = command(core, j.name.clone(), j.params.clone(), f);

    let mut f = Facts::default();
    f.code(codes::INPUT_INVALID, None);
    f.code(codes::NOT_FOUND, None);
    let status = status(format!("{}Status", j.name), vec![param("job", Type::Uuid, false)], f);
    JobIntents { start, status }
}

/// Consents that name `intent` in their `for [..]` list.
pub fn consents_of<'a>(core: &'a Program, intent: &str) -> Vec<&'a Consent> {
    core.forms
        .iter()
        .filter_map(|f| match f {
            Form::Consent(c) if c.intents.iter().any(|i| i == intent) => Some(c),
            _ => None,
        })
        .collect()
}

/// The program's `impersonate` declaration, if it has one.
pub fn impersonation(core: &Program) -> Option<&Impersonate> {
    core.forms.iter().find_map(|f| match f {
        Form::Impersonate(i) => Some(i),
        _ => None,
    })
}

/// Reason a command refuses a caller who is acting as someone else.
pub const IMPERSONATION_FORBIDDEN: &str = "IMPERSONATION_FORBIDDEN";

/// Adds the error of a command that refuses a caller who is acting as someone else, if the program has impersonation.
pub fn refuse_impersonated(core: &Program, f: &mut Facts) {
    if impersonation(core).is_some() {
        f.code(codes::AUTH_FORBIDDEN, Some(IMPERSONATION_FORBIDDEN));
    }
}

/// Whether the statements read out or remove a person's data (`erase`, `export personal data`), which nobody
/// may do while acting as that person.
pub fn touches_personal_data(stmts: &[Stmt]) -> bool {
    stmts.iter().any(|s| match s {
        Stmt::Erase { .. } | Stmt::ExportPersonalData { .. } => true,
        Stmt::When { body, .. } | Stmt::Each { body, .. } => touches_personal_data(body),
        _ => false,
    })
}

/// `Give<Name>Consent()`, `Withdraw<Name>Consent()` and the query `<Name>ConsentStatus()`.
pub struct ConsentIntents {
    pub give: FormIntent,
    pub withdraw: FormIntent,
    pub status: FormIntent,
}

pub fn consent(core: &Program, c: &Consent) -> ConsentIntents {
    // consent is the user's own act: nobody who acts as them may give or withdraw it
    let mut f = Facts::default();
    refuse_impersonated(core, &mut f);
    let give = command(core, format!("Give{}Consent", c.name), Vec::new(), f);
    let mut f = Facts::default();
    refuse_impersonated(core, &mut f);
    let withdraw = command(core, format!("Withdraw{}Consent", c.name), Vec::new(), f);
    let mut f = Facts::default();
    f.code(codes::INPUT_INVALID, None);
    f.code(codes::AUTH_UNAUTHENTICATED, None);
    let status = status(format!("{}ConsentStatus", c.name), Vec::new(), f);
    ConsentIntents { give, withdraw, status }
}

/// `Start<E>Impersonation(target, reason)` and `Stop<E>Impersonation()`.
pub struct ImpersonationIntents {
    pub start: FormIntent,
    pub stop: FormIntent,
}

/// Why a start is refused besides the `by` condition.
pub const IMPERSONATION_NESTED: &str = "IMPERSONATION_NESTED";
pub const IMPERSONATION_SELF: &str = "IMPERSONATION_SELF";
pub const IMPERSONATION_TARGET_PRIVILEGED: &str = "IMPERSONATION_TARGET_PRIVILEGED";

pub fn impersonation_intents(core: &Program, i: &Impersonate) -> ImpersonationIntents {
    let target = param("target", Type::Ref { entity: i.entity.clone() }, false);
    let reason = param("reason", Type::Text { min: Some(1), max: Some(500), trim: true, lower: false, pattern: None }, false);
    let mut f = Facts::default();
    facts::param_loads(std::slice::from_ref(&target), &mut f);
    f.code(codes::AUTH_FORBIDDEN, None);
    for r in [IMPERSONATION_NESTED, IMPERSONATION_SELF, IMPERSONATION_TARGET_PRIVILEGED] {
        f.code(codes::AUTH_FORBIDDEN, Some(r));
    }
    let start = command(core, format!("Start{}Impersonation", i.entity), vec![target, reason], f);
    let mut f = Facts::default();
    f.code(codes::AUTH_UNAUTHENTICATED, None);
    let stop = command(core, format!("Stop{}Impersonation", i.entity), Vec::new(), f);
    ImpersonationIntents { start, stop }
}

/// `Publish<E>(e)` and `Discard<E>Draft(e)`.
pub struct PublishIntents {
    pub publish: FormIntent,
    pub discard: FormIntent,
}

pub fn publishable(core: &Program, entity: &str) -> PublishIntents {
    let subject = param(&lower_first(entity), Type::Ref { entity: entity.to_string() }, false);
    let base = || {
        let mut f = Facts::default();
        facts::param_loads(std::slice::from_ref(&subject), &mut f);
        f.code(codes::AUTH_FORBIDDEN, None);
        f
    };
    // publishing writes only the published copy, which has no constraints of its own to violate
    let mut publish = command(core, format!("Publish{entity}"), vec![subject.clone()], base());
    publish.facts.writes.insert(entity.to_string());
    // discarding writes the values of the published version back, which the entity's constraints judge again
    let mut f = base();
    f.writes.insert(entity.to_string());
    f.code(codes::PRECONDITION_FAILED, Some(NOT_PUBLISHED));
    let discard = command(core, format!("Discard{entity}Draft"), vec![subject], f);
    PublishIntents { publish, discard }
}

/// Reason `Discard<E>Draft` fails with when the row has never been published: there is no version to go back to.
pub const NOT_PUBLISHED: &str = "NOT_PUBLISHED";

/// Every intent a form stands for, in the order a client lists them.
pub fn of_form(core: &Program, form: &Form) -> Vec<FormIntent> {
    match form {
        Form::GrantLink(g) => {
            let i = grant_link(core, g);
            vec![i.issue, i.redeem]
        }
        Form::Verification(v) => {
            let i = verification(core, v);
            vec![i.request, i.verify]
        }
        Form::Approval(a) => {
            let i = approval(core, a);
            vec![i.request, i.approve, i.reject, i.cancel, i.status]
        }
        Form::Job(j) => {
            let i = job(core, j);
            vec![i.start, i.status]
        }
        Form::Consent(c) => {
            let i = consent(core, c);
            vec![i.give, i.withdraw, i.status]
        }
        Form::Impersonate(x) => {
            let i = impersonation_intents(core, x);
            vec![i.start, i.stop]
        }
        _ => Vec::new(),
    }
}
