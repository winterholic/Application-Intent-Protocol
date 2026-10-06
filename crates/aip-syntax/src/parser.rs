//! Recursive-descent parser for `spec/grammar.md`.
//!
//! Layout rules that the EBNF cannot express:
//! - A set-expression alias must be on the same line as the end of its source
//!   and must not be a clause keyword, so `exists membership(a, c)` followed by
//!   `require ...` on the next line is not read as an alias.
//! - Field modifiers continue on a following line only when indented deeper
//!   than the field itself; a line at field indentation starts a new member.

use crate::ast::*;
use crate::diag::{Diagnostic, Span};
use crate::lexer::{Tok, Token, lex};
use aip_ir::codes;

pub fn parse_file(src: &str) -> Result<File, Diagnostic> {
    let toks = lex(src)?;
    let mut p = Parser { toks, pos: 0 };
    let mut decls = Vec::new();
    while !p.at_eof() {
        decls.push(p.decl()?);
    }
    Ok(File { decls })
}

/// Parses a documentation snippet: a whole file, a statement list, or a
/// `do { ... }` block. The file-level error is reported if nothing fits.
pub fn parse_snippet(src: &str) -> Result<File, Diagnostic> {
    let file_err = match parse_file(src) {
        Ok(f) => return Ok(f),
        Err(e) => e,
    };
    let toks = lex(src)?;
    let mut p = Parser { toks: toks.clone(), pos: 0 };
    let mut ok = true;
    while !p.at_eof() {
        if p.stmt().is_err() {
            ok = false;
            break;
        }
    }
    if ok {
        return Ok(File { decls: Vec::new() });
    }
    let mut p = Parser { toks, pos: 0 };
    if p.eat("do") && p.block().is_ok() && p.at_eof() {
        return Ok(File { decls: Vec::new() });
    }
    Err(file_err)
}

/// Words that terminate an expression-level construct and therefore can never
/// be an implicit alias.
const STOP_WORDS: &[&str] = &[
    "where",
    "and",
    "or",
    "not",
    "in",
    "is",
    "has",
    "as",
    "into",
    "by",
    "set",
    "via",
    "else",
    "then",
    "on",
    "do",
    "partial",
    "order",
    "desc",
    "asc",
    "per",
    "for",
    "to",
    "dedupe",
    "when",
    "unless",
    "from",
    "select",
    "sort",
    "page",
    "plan",
    "consistency",
    "touch",
    "group",
    "let",
    "allow",
    "require",
    "emit",
    "returns",
    "over",
    "with",
    "max",
    "insert",
    "update",
    "delete",
    "purge",
    "erase",
    "upsert",
    "toggle",
    "each",
    "at",
    "notify",
    "export",
    "reserve",
    "confirm",
    "release",
    "fetch",
    "run",
    "of",
    "key",
    "fields",
    "window",
    "sharded",
    "within",
    "respecting",
    "digest",
    "expires",
    "uses",
    "issued",
    "grants",
    "scope",
    "approvers",
    "produce",
    "progress",
    "masked",
    "visible",
    "personal",
    "encrypted",
    "tree",
    "sequence",
    "slug",
    "position",
    "counter",
    "variants",
    "unique",
    "exactly",
    "no",
    "capacity",
    "invariant",
    "lifecycle",
    "predicate",
    "track",
    "history",
    "soft",
    "versioned",
    "publishable",
    "tenant",
    "cross",
    "dynamic",
    "repair",
    "limit",
    "cached",
    "idempotent",
    "audited",
    "if",
    "language",
    "sign",
    "retry",
    "disable",
    "events",
    "subject",
    "target",
    "code",
    "deliver",
    "ttl",
    "attempts",
    "resend",
    "catch",
    "tz",
    "every",
];

struct Parser {
    toks: Vec<Token>,
    pos: usize,
}

type PResult<T> = Result<T, Diagnostic>;

impl Parser {
    // ---------- token helpers ----------

    fn tok(&self) -> &Token {
        &self.toks[self.pos.min(self.toks.len() - 1)]
    }

    fn peek_tok(&self, k: usize) -> &Token {
        &self.toks[(self.pos + k).min(self.toks.len() - 1)]
    }

    fn span(&self) -> Span {
        self.tok().span
    }

    fn prev_span(&self) -> Span {
        if self.pos == 0 { self.span() } else { self.toks[self.pos - 1].span }
    }

    fn at_eof(&self) -> bool {
        matches!(self.tok().tok, Tok::Eof)
    }

    fn text_of(t: &Token) -> Option<&str> {
        match &t.tok {
            Tok::Ident(s) => Some(s.as_str()),
            Tok::Punct(p) => Some(p),
            _ => None,
        }
    }

    fn at(&self, s: &str) -> bool {
        Self::text_of(self.tok()) == Some(s)
    }

    fn at_k(&self, k: usize, s: &str) -> bool {
        Self::text_of(self.peek_tok(k)) == Some(s)
    }

    fn eat(&mut self, s: &str) -> bool {
        if self.at(s) {
            self.pos += 1;
            true
        } else {
            false
        }
    }

    fn describe(t: &Token) -> String {
        match &t.tok {
            Tok::Ident(s) => format!("'{s}'"),
            Tok::Punct(p) => format!("'{p}'"),
            Tok::Int(n) => format!("number {n}"),
            Tok::Decimal(d) => format!("number {d}"),
            Tok::Str(_) => "string".into(),
            Tok::Duration(..) => "duration".into(),
            Tok::Size(_) => "size".into(),
            Tok::TimeOfDay(..) => "time of day".into(),
            Tok::Regex(_) => "regex".into(),
            Tok::Eof => "end of file".into(),
        }
    }

    fn err<T>(&self, msg: impl Into<String>) -> PResult<T> {
        Err(Diagnostic::error(codes::E100, msg, self.span()))
    }

    fn expect(&mut self, s: &str) -> PResult<Span> {
        if self.at(s) {
            let sp = self.span();
            self.pos += 1;
            Ok(sp)
        } else {
            self.err(format!("expected '{s}' but found {}", Self::describe(self.tok())))
        }
    }

    fn ident(&mut self) -> PResult<Ident> {
        match &self.tok().tok {
            Tok::Ident(s) => {
                let id = Ident { name: s.clone(), span: self.span() };
                self.pos += 1;
                Ok(id)
            }
            _ => self.err(format!("expected a name but found {}", Self::describe(self.tok()))),
        }
    }

    fn is_ident(&self) -> bool {
        matches!(self.tok().tok, Tok::Ident(_))
    }

    fn code(&mut self) -> PResult<Ident> {
        let id = self.ident()?;
        let ok = id.name.chars().next().is_some_and(|c| c.is_ascii_uppercase())
            && id.name.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || c == '_');
        if !ok {
            return Err(Diagnostic::error(codes::E100, format!("error code '{}' must be UPPER_SNAKE_CASE", id.name), id.span));
        }
        Ok(id)
    }

    fn int(&mut self) -> PResult<i64> {
        match self.tok().tok {
            Tok::Int(n) => {
                self.pos += 1;
                Ok(n)
            }
            _ => self.err(format!("expected a number but found {}", Self::describe(self.tok()))),
        }
    }

    fn string(&mut self) -> PResult<String> {
        match &self.tok().tok {
            Tok::Str(s) => {
                let s = s.clone();
                self.pos += 1;
                Ok(s)
            }
            _ => self.err(format!("expected a string but found {}", Self::describe(self.tok()))),
        }
    }

    fn duration(&mut self) -> PResult<Duration> {
        match self.tok().tok {
            Tok::Duration(n, unit) => {
                self.pos += 1;
                Ok(Duration { n, unit })
            }
            _ => self
                .err::<Duration>(format!("expected a duration like 10m or 7d but found {}", Self::describe(self.tok())))
                .map_err(|d| d.with_help("durations are written 30s, 10m, 2h, 7d, 2w, 6mo, 1y")),
        }
    }

    fn qualname(&mut self) -> PResult<QualName> {
        let first = self.ident()?;
        let span = first.span;
        let mut parts = vec![first];
        while self.at(".") && matches!(self.peek_tok(1).tok, Tok::Ident(_)) {
            self.pos += 1;
            parts.push(self.ident()?);
        }
        let end = self.prev_span();
        Ok(QualName { parts, span: span.to(end) })
    }

    fn ident_list_bracket(&mut self) -> PResult<Vec<Ident>> {
        self.expect("[")?;
        let mut out = Vec::new();
        if !self.at("]") {
            loop {
                out.push(self.ident()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect("]")?;
        Ok(out)
    }

    fn version_word(&mut self) -> PResult<i64> {
        // `v2` lexes as one identifier
        let id = self.ident()?;
        id.name
            .strip_prefix('v')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| Diagnostic::error(codes::E100, format!("expected a version like v2 but found '{}'", id.name), id.span))
    }

    fn at_version_word(&self) -> bool {
        matches!(&self.tok().tok, Tok::Ident(s) if s.len() > 1 && s.starts_with('v') && s[1..].chars().all(|c| c.is_ascii_digit()))
    }

    // ---------- declarations ----------

    fn decl(&mut self) -> PResult<Decl> {
        let start = self.span();
        let kw = match &self.tok().tok {
            Tok::Ident(s) => s.clone(),
            _ => return self.err(format!("expected a declaration but found {}", Self::describe(self.tok()))),
        };
        match kw.as_str() {
            "use" => {
                self.pos += 1;
                Ok(Decl::Use(self.qualname()?))
            }
            "actor" => self.actor(start),
            "enum" => self.enum_decl(start),
            "record" => self.record(start),
            "entity" => self.entity(start),
            "relation" => self.relation(start),
            "fn" => self.fn_decl(start),
            "event" => self.event(start),
            "upcast" => self.upcast(start),
            // `cross tenant` without `internal` parses so that analysis can say why it is refused (AIP-E315)
            "internal" | "cross" | "drafts" => {
                let internal = self.eat("internal");
                let cross_tenant = self.eat("cross");
                if cross_tenant {
                    self.expect("tenant")?;
                }
                let drafts = self.eat("drafts");
                if cross_tenant && !internal && !drafts && ["consume", "on", "schedule", "rule"].iter().any(|k| self.at(k)) {
                    return self.cross_reaction(start);
                }
                if self.at("query") {
                    let mut q = self.query(start, internal)?;
                    q.cross_tenant = cross_tenant;
                    q.drafts = drafts;
                    Ok(Decl::Query(q))
                } else if self.at("command") && !drafts {
                    let mut c = self.command(start, internal)?;
                    c.cross_tenant = cross_tenant;
                    Ok(Decl::Command(c))
                } else if drafts {
                    self.err("'drafts' must be followed by 'query'")
                } else {
                    self.err("'internal' must be followed by 'query' or 'command'; 'cross tenant' by one of those, 'on', 'schedule', 'rule' or 'consume'")
                }
            }
            "query" => self.query(start, false).map(Decl::Query),
            "command" => self.command(start, false).map(Decl::Command),
            "subscribe" => self.subscribe(start),
            "webhook" => self.webhook(start),
            "consume" => self.consume(start),
            "on" => self.on_event(start),
            "schedule" => self.schedule(start),
            "retain" => self.retain(start),
            "rule" => self.rule(start),
            "projection" => self.projection(start),
            "search" => self.search(start),
            "job" => self.job(start),
            "verification" => self.verification(start),
            "grant" => self.grant_link(start),
            "approval" => self.approval(start),
            "expose" => self.expose(start),
            "outbound" => self.outbound(start),
            "consent" => self.consent(start),
            "config" => self.config(start),
            "flag" => self.flag(start),
            "impersonate" => self.impersonate(start),
            "removed" => {
                self.pos += 1;
                self.expect("entity")?;
                let name = self.ident()?;
                Ok(Decl::Removed(RemovedDecl { name, span: start.to(self.prev_span()) }))
            }
            "migration" => {
                self.pos += 1;
                let name = self.ident()?;
                let body = self.block()?;
                Ok(Decl::Migration(MigrationDecl { name, body, span: start.to(self.prev_span()) }))
            }
            other => Err(Diagnostic::error(codes::E100, format!("unknown declaration '{other}'"), start).with_help(
                "declarations: use actor enum record entity relation fn event upcast query command subscribe webhook consume on schedule retain rule projection search job verification grant approval expose outbound consent config flag impersonate migration",
            )),
        }
    }

    fn call_target(&mut self) -> PResult<CallTarget> {
        let name = self.qualname()?;
        let args = if self.at("(") { Some(self.args()?) } else { None };
        Ok(CallTarget { name, args })
    }

    fn actor(&mut self, start: Span) -> PResult<Decl> {
        self.expect("actor")?;
        let name = self.ident()?;
        self.expect("via")?;
        let via = self.call_target()?;
        let mut scopes = Vec::new();
        if self.eat("scopes") {
            self.expect("[")?;
            loop {
                scopes.push(self.qualname()?);
                if !self.eat(",") {
                    break;
                }
            }
            self.expect("]")?;
        }
        let mut superuser = None;
        if self.eat("superuser") {
            self.expect("when")?;
            superuser = Some(self.expr()?);
            self.expect("audited").map_err(|d| d.with_help("a superuser bypass must be audited: '... superuser when <cond> audited'"))?;
        }
        Ok(Decl::Actor(ActorDecl { name, via, scopes, superuser, span: start.to(self.prev_span()) }))
    }

    fn enum_decl(&mut self, start: Span) -> PResult<Decl> {
        self.expect("enum")?;
        let name = self.ident()?;
        let ordered = self.eat("ordered");
        self.expect("{")?;
        let mut values = Vec::new();
        while !self.at("}") {
            values.push(self.ident()?);
            if ordered && !self.at("}") {
                self.expect("<").map_err(|d| d.with_help("ordered enums list values from low to high: { A < B < C }"))?;
            }
        }
        self.expect("}")?;
        Ok(Decl::Enum(EnumDecl { name, ordered, values, span: start.to(self.prev_span()) }))
    }

    fn record(&mut self, start: Span) -> PResult<Decl> {
        self.expect("record")?;
        let name = self.ident()?;
        self.expect("{")?;
        let mut fields = Vec::new();
        let mut checks = Vec::new();
        while !self.eat("}") {
            if self.at("check") && !self.at_k(1, ":") {
                let cs = self.span();
                self.pos += 1;
                let when = if self.eat("when") {
                    let w = self.expr()?;
                    self.expect(":")?;
                    Some(w)
                } else {
                    None
                };
                let cond = self.expr()?;
                self.expect("else")?;
                let code = self.code()?;
                checks.push(RecordCheck { when, cond, code, span: cs.to(self.prev_span()) });
            } else {
                fields.push(self.field_decl()?);
            }
        }
        Ok(Decl::Record(RecordDecl { name, fields, checks, span: start.to(self.prev_span()) }))
    }

    fn entity(&mut self, start: Span) -> PResult<Decl> {
        self.expect("entity")?;
        let name = self.ident()?;
        let was = if self.eat("was") { Some(self.ident()?) } else { None };
        let mut personal = false;
        let mut access_audited = false;
        if self.eat("personal") {
            personal = true;
            if self.eat("access") {
                self.expect("audited")?;
                access_audited = true;
            }
        }
        self.expect("{")?;
        let mut members = Vec::new();
        while !self.eat("}") {
            members.push(self.entity_member()?);
        }
        Ok(Decl::Entity(EntityDecl { name, was, personal, access_audited, members, span: start.to(self.prev_span()) }))
    }

    fn entity_member(&mut self) -> PResult<EntityMember> {
        if self.is_ident() && self.at_k(1, ":") {
            return Ok(EntityMember::Field(self.field_decl()?));
        }
        let start = self.span();
        let kw = match &self.tok().tok {
            Tok::Ident(s) => s.clone(),
            _ => return self.err(format!("expected a field or entity clause but found {}", Self::describe(self.tok()))),
        };
        match kw.as_str() {
            "removed" => {
                self.pos += 1;
                self.expect("field")?;
                Ok(EntityMember::RemovedField(self.ident()?))
            }
            "unique" => {
                self.pos += 1;
                self.expect("(")?;
                let mut cols = vec![self.ident()?];
                while self.eat(",") {
                    cols.push(self.ident()?);
                }
                self.expect(")")?;
                let filter = if self.eat("where") { Some(self.expr()?) } else { None };
                let code = if self.eat("else") { Some(self.code()?) } else { None };
                Ok(EntityMember::Constraint(Constraint::Unique { cols, filter, code, span: start.to(self.prev_span()) }))
            }
            "exactly" | "at" => {
                self.pos += 1;
                let kind = if kw == "exactly" {
                    CardKind::ExactlyOne
                } else if self.eat("most") {
                    CardKind::AtMostOne
                } else {
                    self.expect("least")?;
                    CardKind::AtLeastOne
                };
                self.expect("one")?;
                self.expect("where")?;
                let filter = self.expr()?;
                self.expect("per")?;
                let per = self.ident()?;
                let repair = if self.eat("repair") {
                    self.expect("on")?;
                    self.expect("erase")?;
                    self.expect("do")?;
                    Some(self.block()?)
                } else {
                    None
                };
                Ok(EntityMember::Constraint(Constraint::Cardinality { kind, filter, per, repair, span: start.to(self.prev_span()) }))
            }
            "no" => {
                self.pos += 1;
                self.expect("overlap")?;
                self.expect("(")?;
                let range = self.ident()?;
                self.expect(")")?;
                self.expect("per")?;
                let per = self.ident()?;
                let code = if self.eat("else") { Some(self.code()?) } else { None };
                Ok(EntityMember::Constraint(Constraint::NoOverlap { range, per, code, span: start.to(self.prev_span()) }))
            }
            "capacity" => {
                self.pos += 1;
                self.expect("count")?;
                self.expect("(")?;
                let count = self.set_expr()?;
                self.expect(")")?;
                self.expect("<=")?;
                let limit = self.expr()?;
                self.expect("else")?;
                let code = self.code()?;
                Ok(EntityMember::Constraint(Constraint::Capacity { count, limit, code, span: start.to(self.prev_span()) }))
            }
            "invariant" => {
                self.pos += 1;
                let name = self.ident()?;
                self.expect(":")?;
                let cond = self.expr()?;
                Ok(EntityMember::Constraint(Constraint::Invariant { name, cond, span: start.to(self.prev_span()) }))
            }
            "lifecycle" => {
                self.pos += 1;
                let field = self.ident()?;
                self.expect("{")?;
                let mut transitions = Vec::new();
                while !self.eat("}") {
                    let ts = self.span();
                    let mut from = vec![self.ident()?];
                    while self.eat(",") {
                        from.push(self.ident()?);
                    }
                    self.expect("->")?;
                    let mut to = vec![self.ident()?];
                    while self.eat(",") {
                        to.push(self.ident()?);
                    }
                    transitions.push(Transition { from, to, span: ts.to(self.prev_span()) });
                }
                Ok(EntityMember::Lifecycle(Lifecycle { field, transitions, span: start.to(self.prev_span()) }))
            }
            "visible" => {
                self.pos += 1;
                self.expect("to")?;
                self.expect("actor")?;
                let unless = if self.eat("unless") {
                    true
                } else {
                    self.expect("when")?;
                    false
                };
                let cond = self.expr()?;
                Ok(EntityMember::Visibility(Visibility { unless, cond, span: start.to(self.prev_span()) }))
            }
            "predicate" => {
                self.pos += 1;
                let name = self.ident()?;
                self.expect("=")?;
                let body = self.expr()?;
                Ok(EntityMember::Predicate(PredicateDecl { name, body, span: start.to(self.prev_span()) }))
            }
            "track" => {
                self.pos += 1;
                let (mut created, mut updated) = (false, false);
                loop {
                    if self.eat("created") {
                        created = true;
                    } else {
                        self.expect("updated")?;
                        updated = true;
                    }
                    if !self.eat(",") {
                        break;
                    }
                }
                let by_actor = if self.eat("by") {
                    self.expect("actor")?;
                    true
                } else {
                    false
                };
                Ok(EntityMember::Trait(EntityTrait::Track { created, updated, by_actor, span: start.to(self.prev_span()) }))
            }
            "history" => {
                self.pos += 1;
                Ok(EntityMember::Trait(EntityTrait::History(start)))
            }
            "soft" => {
                self.pos += 1;
                self.expect("delete")?;
                let retain = if self.eat("retain") { Some(self.duration()?) } else { None };
                Ok(EntityMember::Trait(EntityTrait::SoftDelete { retain, span: start.to(self.prev_span()) }))
            }
            "versioned" => {
                self.pos += 1;
                Ok(EntityMember::Trait(EntityTrait::Versioned(start)))
            }
            "publishable" => {
                self.pos += 1;
                let by = if self.eat("by") { Some(self.expr()?) } else { None };
                Ok(EntityMember::Trait(EntityTrait::Publishable { by, span: start.to(self.prev_span()) }))
            }
            "tenant" => {
                self.pos += 1;
                self.expect("via")?;
                let via = self.qualname()?;
                Ok(EntityMember::Trait(EntityTrait::Tenant { via, span: start.to(self.prev_span()) }))
            }
            "dynamic" => {
                self.pos += 1;
                self.expect("schema")?;
                self.expect("from")?;
                let from = self.ident()?;
                Ok(EntityMember::Trait(EntityTrait::DynamicSchema { from, span: start.to(self.prev_span()) }))
            }
            other => Err(Diagnostic::error(codes::E100, format!("unknown entity clause '{other}'"), start)
                .with_help("a field is written 'name: Type'; clauses are unique, removed field, exactly/at most/at least one, no overlap, capacity, invariant, lifecycle, visible to actor, predicate, track, history, soft delete, versioned, publishable, tenant via, dynamic schema from")),
        }
    }

    fn field_decl(&mut self) -> PResult<FieldDecl> {
        let name = self.ident()?;
        let (fline, fcol) = (name.span.line, name.span.col);
        self.expect(":")?;
        let ty = self.type_expr()?;
        let default = if self.eat("=") { Some(self.expr()?) } else { None };
        let mut mods = Vec::new();
        loop {
            let t = self.span();
            let continues = t.line == fline || t.col > fcol;
            if !continues {
                break;
            }
            let Some(m) = self.field_mod()? else { break };
            mods.push(m);
        }
        Ok(FieldDecl { span: name.span.to(self.prev_span()), name, ty, default, mods })
    }

    fn field_mod(&mut self) -> PResult<Option<FieldMod>> {
        let start = self.span();
        let kw = match &self.tok().tok {
            Tok::Ident(s) => s.clone(),
            _ => return Ok(None),
        };
        // `name:` on a continuation line is a new field, not a modifier
        if self.at_k(1, ":") {
            return Ok(None);
        }
        let m = match kw.as_str() {
            "was" => {
                self.pos += 1;
                FieldMod::Was(self.ident()?)
            }
            "personal" => {
                self.pos += 1;
                FieldMod::Personal(start)
            }
            "encrypted" => {
                self.pos += 1;
                FieldMod::Encrypted(start)
            }
            "visible" => {
                self.pos += 1;
                self.expect("to")?;
                FieldMod::VisibleTo(self.expr()?)
            }
            "masked" => {
                self.pos += 1;
                self.expect("unless")?;
                let unless = self.expr()?;
                self.expect("as")?;
                FieldMod::Masked { unless, with: self.call_target()? }
            }
            "on" => {
                self.pos += 1;
                if self.eat("delete") {
                    FieldMod::OnDelete(self.ref_policy()?)
                } else {
                    self.expect("erase")?;
                    FieldMod::OnErase(self.ref_policy()?)
                }
            }
            "via" => {
                self.pos += 1;
                FieldMod::Via(self.ident()?)
            }
            "tree" => {
                self.pos += 1;
                let max_depth = if self.eat("max") {
                    self.expect("depth")?;
                    Some(self.int()?)
                } else {
                    None
                };
                FieldMod::Tree { max_depth, span: start.to(self.prev_span()) }
            }
            "sequence" => {
                self.pos += 1;
                self.expect("per")?;
                let per = self.qualname()?;
                self.expect("format")?;
                let format = self.string()?;
                FieldMod::Sequence { per, format, span: start.to(self.prev_span()) }
            }
            "slug" => {
                self.pos += 1;
                self.expect("from")?;
                let from = self.ident()?;
                self.expect("unique")?;
                let per = if self.eat("per") { Some(self.qualname()?) } else { None };
                FieldMod::Slug { from, per, span: start.to(self.prev_span()) }
            }
            "position" => {
                self.pos += 1;
                self.expect("within")?;
                FieldMod::Position { within: self.qualname()?, span: start.to(self.prev_span()) }
            }
            "counter" => {
                self.pos += 1;
                self.expect("via")?;
                let via = self.ident()?;
                let mut opts = Vec::new();
                loop {
                    if self.eat("dedupe") {
                        self.expect("by")?;
                        let by = self.ident()?;
                        self.expect("within")?;
                        opts.push(CounterOpt::Dedupe { by, within: self.duration()? });
                    } else if self.eat("window") {
                        opts.push(CounterOpt::Window(self.duration()?));
                    } else if self.eat("sharded") {
                        opts.push(CounterOpt::Sharded(self.int()?));
                    } else {
                        break;
                    }
                }
                FieldMod::Counter { via, opts, span: start.to(self.prev_span()) }
            }
            "variants" => {
                self.pos += 1;
                self.expect("{")?;
                let mut vs = Vec::new();
                while !self.eat("}") {
                    let n = self.ident()?;
                    self.expect(":")?;
                    vs.push((n, self.string()?));
                    self.eat(",");
                }
                FieldMod::Variants(vs)
            }
            _ => return Ok(None),
        };
        Ok(Some(m))
    }

    fn ref_policy(&mut self) -> PResult<RefPolicy> {
        let s = self.span();
        if self.eat("cascade") {
            Ok(RefPolicy::Cascade(s))
        } else if self.eat("restrict") {
            Ok(RefPolicy::Restrict(s))
        } else if self.eat("set") {
            self.expect("null")?;
            Ok(RefPolicy::SetNull(s))
        } else if self.eat("anonymize") {
            Ok(RefPolicy::Anonymize(s))
        } else if self.eat("reassign") {
            self.expect("to")?;
            Ok(RefPolicy::Reassign(self.expr()?))
        } else {
            self.err("expected cascade, restrict, set null, anonymize or reassign to <expr>")
        }
    }

    // ---------- types ----------

    fn type_expr(&mut self) -> PResult<TypeExpr> {
        let start = self.span();
        let kind = if self.eat("ref") {
            let mut alts = vec![self.ident()?];
            while self.eat("|") {
                alts.push(self.ident()?);
            }
            if alts.len() < 2 {
                return Err(Diagnostic::error(codes::E100, "'ref' introduces a union of at least two entities: ref A | B", start));
            }
            TypeKind::Union(alts)
        } else {
            let name = self.qualname()?;
            let base = name.parts[0].clone();
            if name.parts.len() == 1 && self.at("<") {
                self.pos += 1;
                let arg = Box::new(self.type_expr()?);
                self.expect(">")?;
                let max = if self.eat("max") { Some(self.int()? as u64) } else { None };
                if matches!(base.name.as_str(), "Set" | "List") && max.is_none() {
                    return Err(Diagnostic::error(codes::E100, format!("{}<...> needs an upper bound", base.name), start)
                        .with_help(format!("write {}<T> max 100; unbounded collections are not expressible", base.name)));
                }
                TypeKind::Generic { base, arg, max }
            } else if name.parts.len() == 1 && self.at("(") {
                self.pos += 1;
                let mut opts = Vec::new();
                if !self.at(")") {
                    loop {
                        opts.push(self.refine_opt()?);
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                self.expect(")")?;
                TypeKind::Refined { base, opts }
            } else if name.parts.len() == 1 && base.name == "Json" && self.at("validated") {
                self.pos += 1;
                self.expect("by")?;
                TypeKind::JsonValidated(self.qualname()?)
            } else if name.parts.len() == 1 && self.at("[") && self.at_k(1, "]") {
                self.pos += 2;
                TypeKind::Many(base)
            } else {
                TypeKind::Name(name)
            }
        };
        let optional = self.eat("?");
        Ok(TypeExpr { kind, optional, span: start.to(self.prev_span()) })
    }

    fn refine_opt(&mut self) -> PResult<RefineOpt> {
        match self.tok().tok.clone() {
            Tok::Int(n) => {
                self.pos += 1;
                if self.eat("..") {
                    let hi = if let Tok::Int(m) = self.tok().tok {
                        self.pos += 1;
                        Some(m)
                    } else {
                        None
                    };
                    Ok(RefineOpt::Range(Some(n), hi))
                } else {
                    Ok(RefineOpt::Int(n))
                }
            }
            Tok::Punct("..") => {
                self.pos += 1;
                let hi = self.int()?;
                Ok(RefineOpt::Range(None, Some(hi)))
            }
            Tok::Ident(w) if w == "max" => {
                self.pos += 1;
                match self.tok().tok {
                    Tok::Size(n) => {
                        self.pos += 1;
                        Ok(RefineOpt::Max(n))
                    }
                    _ => self.err("expected a size like 10MB after 'max'"),
                }
            }
            Tok::Ident(w) if w == "types" => {
                self.pos += 1;
                Ok(RefineOpt::Types(self.ident_list_bracket()?))
            }
            Tok::Ident(w) if w == "matches" => {
                self.pos += 1;
                match self.tok().tok.clone() {
                    Tok::Regex(r) => {
                        self.pos += 1;
                        Ok(RefineOpt::Matches(r))
                    }
                    _ => self.err("expected /regex/ after 'matches'"),
                }
            }
            Tok::Ident(_) => {
                let k = self.ident()?;
                if self.eat(":") { Ok(RefineOpt::KeyValue(k, self.ident()?)) } else { Ok(RefineOpt::Word(k)) }
            }
            _ => self.err(format!("unexpected {} in type options", Self::describe(self.tok()))),
        }
    }

    fn params(&mut self) -> PResult<Vec<Param>> {
        self.expect("(")?;
        let mut out = Vec::new();
        if !self.at(")") {
            loop {
                out.push(self.param()?);
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect(")")?;
        Ok(out)
    }

    fn param(&mut self) -> PResult<Param> {
        let name = self.ident()?;
        self.expect(":")?;
        let ty = self.type_expr()?;
        let default = if self.eat("=") { Some(self.expr()?) } else { None };
        Ok(Param { span: name.span.to(self.prev_span()), name, ty, default })
    }

    fn relation(&mut self, start: Span) -> PResult<Decl> {
        self.expect("relation")?;
        let name = self.ident()?;
        let params = self.params()?;
        let result = if self.eat(":") { Some(self.ident()?) } else { None };
        self.expect("=")?;
        let body = self.expr()?;
        Ok(Decl::Relation(RelationDecl { name, params, result, body, span: start.to(self.prev_span()) }))
    }

    fn fn_decl(&mut self, start: Span) -> PResult<Decl> {
        self.expect("fn")?;
        let name = self.ident()?;
        let params = self.params()?;
        self.expect(":")?;
        let ret = self.type_expr()?;
        let body = if self.eat("wasm") {
            FnBody::Wasm(self.string()?)
        } else {
            self.expect("=")?;
            FnBody::Expr(self.expr()?)
        };
        Ok(Decl::Fn(FnDecl { name, params, ret, body, span: start.to(self.prev_span()) }))
    }

    fn event(&mut self, start: Span) -> PResult<Decl> {
        self.expect("event")?;
        let name = self.ident()?;
        let version = if self.at_version_word() { Some(self.version_word()?) } else { None };
        self.expect("{")?;
        let mut fields = Vec::new();
        while !self.eat("}") {
            let n = self.ident()?;
            self.expect(":")?;
            fields.push((n, self.type_expr()?));
            self.eat(",");
        }
        Ok(Decl::Event(EventDecl { name, version, fields, span: start.to(self.prev_span()) }))
    }

    fn upcast(&mut self, start: Span) -> PResult<Decl> {
        self.expect("upcast")?;
        let event = self.ident()?;
        let from = self.version_word()?;
        self.expect("->")?;
        let to = self.version_word()?;
        self.expect("with")?;
        let with = self.field_assigns()?;
        Ok(Decl::Upcast(UpcastDecl { event, from, to, with, span: start.to(self.prev_span()) }))
    }

    // ---------- intents ----------

    fn rates(&mut self) -> PResult<Vec<Rate>> {
        let mut out = Vec::new();
        loop {
            let s = self.span();
            let n = self.int()?;
            self.expect("per")?;
            let per = self.duration()?;
            self.expect("per")?;
            let key = if self.eat("actor") {
                RateKey::Actor
            } else if self.eat("client") {
                RateKey::Client
            } else {
                RateKey::Path(self.qualname()?)
            };
            out.push(Rate { n, per, key, span: s.to(self.prev_span()) });
            if !self.eat(",") {
                break;
            }
        }
        Ok(out)
    }

    fn clause_order(&self, order: &[&str], last: &mut usize, name: &str) -> PResult<()> {
        let idx = order.iter().position(|c| *c == name).unwrap_or(0);
        if idx < *last {
            return Err(Diagnostic::error(codes::E100, format!("'{name}' must come before '{}'", order[*last]), self.span())
                .with_help(format!("canonical clause order: {}", order.join(" → "))));
        }
        *last = idx;
        Ok(())
    }

    fn let_clause(&mut self) -> PResult<Let> {
        let s = self.expect("let")?;
        let name = self.ident()?;
        self.expect("=")?;
        let value = self.expr()?;
        let code = if self.eat("else") { Some(self.code()?) } else { None };
        Ok(Let { name, value, code, span: s.to(self.prev_span()) })
    }

    fn allow(&mut self) -> PResult<Allow> {
        let s = self.expect("allow")?;
        let cond = self.expr()?;
        let code = if self.eat("else") { Some(self.code()?) } else { None };
        Ok(Allow { cond, code, span: s.to(self.prev_span()) })
    }

    fn require(&mut self) -> PResult<Require> {
        let s = self.expect("require")?;
        let when = if self.eat("when") {
            let w = self.expr()?;
            self.expect(":")?;
            Some(w)
        } else {
            None
        };
        let cond = self.expr()?;
        self.expect("else").map_err(|d| d.with_help("every require needs a failure code: require <cond> else SOME_CODE"))?;
        let code = self.code()?;
        Ok(Require { when, cond, code, span: s.to(self.prev_span()) })
    }

    fn query(&mut self, start: Span, internal: bool) -> PResult<QueryDecl> {
        self.expect("query")?;
        let name = self.ident()?;
        let params = if self.at("(") { self.params()? } else { Vec::new() };
        let mut cached = None;
        let mut limits = Vec::new();
        loop {
            if self.eat("cached") {
                let d = self.duration()?;
                let per = if self.eat("per") {
                    let id = self.ident()?;
                    if id.name == "actor" && self.at("-") && self.at_k(1, "class") {
                        self.pos += 2;
                        Some(Ident { name: "actor-class".into(), span: id.span.to(self.prev_span()) })
                    } else {
                        Some(id)
                    }
                } else {
                    None
                };
                cached = Some((d, per));
            } else if self.eat("limit") {
                limits.extend(self.rates()?);
            } else {
                break;
            }
        }
        self.expect("{")?;
        let order = ["let", "allow", "fetch", "from", "where", "group", "sort", "page", "plan", "consistency", "select", "touch"];
        let mut last = 0;
        let mut q = QueryDecl {
            internal,
            drafts: false,
            cross_tenant: false,
            name,
            params,
            cached,
            limits,
            lets: Vec::new(),
            allow: None,
            fetches: Vec::new(),
            from: None,
            filter: None,
            group_by: Vec::new(),
            sort: None,
            page: None,
            plan: None,
            consistency: None,
            select: Selection { items: Vec::new(), span: start },
            touches: Vec::new(),
            span: start,
        };
        let mut has_select = false;
        while !self.eat("}") {
            let kw = match &self.tok().tok {
                Tok::Ident(s) if order.contains(&s.as_str()) => s.clone(),
                _ => {
                    return self
                        .err(format!("unexpected {} in query", Self::describe(self.tok())))
                        .map_err(|d: Diagnostic| d.with_help(format!("query clauses: {}", order.join(" → "))));
                }
            };
            self.clause_order(&order, &mut last, &kw)?;
            match kw.as_str() {
                "let" => q.lets.push(self.let_clause()?),
                "allow" => {
                    if q.allow.is_some() {
                        return self.err("duplicate 'allow'");
                    }
                    q.allow = Some(self.allow()?);
                }
                "fetch" => {
                    self.pos += 1;
                    let call = self.call_expr()?;
                    self.expect("as")?;
                    q.fetches.push((call, self.ident()?));
                }
                "from" => {
                    self.pos += 1;
                    q.from = Some(self.source_clause()?);
                }
                "where" => {
                    self.pos += 1;
                    q.filter = Some(self.expr()?);
                }
                "group" => {
                    self.pos += 1;
                    self.expect("by")?;
                    loop {
                        q.group_by.push(self.postfix()?);
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                "sort" => {
                    self.pos += 1;
                    self.expect("by")?;
                    q.sort = Some(self.sort()?);
                }
                "page" => {
                    let s = self.span();
                    self.pos += 1;
                    let size = self.int()?;
                    self.expect("by")?;
                    let offset_max_page = if self.eat("keyset") {
                        None
                    } else {
                        self.expect("offset")?;
                        self.expect("max")?;
                        self.expect("page")?;
                        Some(self.int()?)
                    };
                    q.page = Some(Page { size, offset_max_page, span: s.to(self.prev_span()) });
                }
                "plan" => {
                    self.pos += 1;
                    q.plan = Some(self.ident()?);
                }
                "consistency" => {
                    self.pos += 1;
                    q.consistency = Some(self.ident()?);
                }
                "select" => {
                    self.pos += 1;
                    q.select = self.selection()?;
                    has_select = true;
                }
                "touch" => {
                    self.pos += 1;
                    q.touches.push(self.postfix()?);
                }
                _ => unreachable!(),
            }
        }
        if !has_select {
            return Err(Diagnostic::error(codes::E100, format!("query {} has no 'select'", q.name.name), q.name.span));
        }
        q.span = start.to(self.prev_span());
        Ok(q)
    }

    fn source_clause(&mut self) -> PResult<FromClause> {
        let name = self.qualname()?;
        if self.at("(") {
            let args = self.args()?;
            let span = name.span.to(self.prev_span());
            let alias = self.ident()?;
            return Ok(FromClause::Call { call: CallExpr { callee: name, args, span }, alias });
        }
        if name.parts.len() == 1 {
            if let Some(alias) = self.opt_alias() {
                return Ok(FromClause::Entity { entity: name.parts[0].clone(), alias });
            }
            return Ok(FromClause::Param(name.parts[0].clone()));
        }
        Err(Diagnostic::error(codes::E100, "expected 'from Entity alias', 'from param' or 'from call(...) alias'", name.span))
    }

    fn sort(&mut self) -> PResult<Sort> {
        if self.is_ident() && self.at_k(1, "of") {
            let param = self.ident()?;
            self.expect("of")?;
            self.expect("{")?;
            let mut cases = Vec::new();
            while !self.eat("}") {
                let key = self.ident()?;
                self.expect(":")?;
                cases.push((key, self.sort_keys()?));
            }
            return Ok(Sort::ByParam { param, cases });
        }
        Ok(Sort::Keys(self.sort_keys()?))
    }

    fn sort_keys(&mut self) -> PResult<Vec<SortKey>> {
        let mut keys = Vec::new();
        loop {
            let expr = self.add()?;
            let desc = if self.eat("desc") {
                true
            } else {
                self.expect("asc").map_err(|d| d.with_help("every sort key needs an explicit direction: asc or desc"))?;
                false
            };
            keys.push(SortKey { expr, desc });
            if !self.eat(",") {
                break;
            }
        }
        Ok(keys)
    }

    fn selection(&mut self) -> PResult<Selection> {
        let s = self.expect("{")?;
        let mut items = Vec::new();
        while !self.eat("}") {
            let name = self.ident()?;
            let value = if self.eat(":") { Some(self.expr()?) } else { None };
            let sub = if self.at("{") { Some(self.selection()?) } else { None };
            items.push(SelItem { name, value, sub });
        }
        Ok(Selection { items, span: s.to(self.prev_span()) })
    }

    fn command(&mut self, start: Span, internal: bool) -> PResult<CommandDecl> {
        self.expect("command")?;
        let name = self.ident()?;
        let params = if self.at("(") { self.params()? } else { Vec::new() };
        let mut c = CommandDecl {
            internal,
            cross_tenant: false,
            name,
            params,
            idempotent: None,
            audited: false,
            limits: Vec::new(),
            lets: Vec::new(),
            allow: None,
            requires: Vec::new(),
            body: None,
            emits: Vec::new(),
            returns: None,
            span: start,
        };
        loop {
            if self.eat("idempotent") {
                c.idempotent = Some(if self.eat("by") { Some(self.expr()?) } else { None });
            } else if self.eat("audited") {
                c.audited = true;
            } else if self.eat("limit") {
                c.limits.extend(self.rates()?);
            } else {
                break;
            }
        }
        self.expect("{")?;
        let order = ["let", "allow", "require", "do", "emit", "returns"];
        let mut last = 0;
        while !self.eat("}") {
            let kw = match &self.tok().tok {
                Tok::Ident(s) if order.contains(&s.as_str()) => s.clone(),
                _ => {
                    return self
                        .err(format!("unexpected {} in command", Self::describe(self.tok())))
                        .map_err(|d: Diagnostic| d.with_help(format!("command clauses: {}", order.join(" → "))));
                }
            };
            self.clause_order(&order, &mut last, &kw)?;
            match kw.as_str() {
                "let" => c.lets.push(self.let_clause()?),
                "allow" => {
                    if c.allow.is_some() {
                        return self.err("duplicate 'allow'");
                    }
                    c.allow = Some(self.allow()?);
                }
                "require" => c.requires.push(self.require()?),
                "do" => {
                    if c.body.is_some() {
                        return self.err("duplicate 'do'");
                    }
                    self.pos += 1;
                    c.body = Some(self.block()?);
                }
                "emit" => c.emits.push(self.emit()?),
                "returns" => {
                    self.pos += 1;
                    let e = self.expr()?;
                    let sel = if self.at("{") { Some(self.selection()?) } else { None };
                    c.returns = Some((e, sel));
                }
                _ => unreachable!(),
            }
        }
        c.span = start.to(self.prev_span());
        Ok(c)
    }

    fn emit(&mut self) -> PResult<Emit> {
        let s = self.expect("emit")?;
        let event = self.ident()?;
        self.expect("{")?;
        let fields = if self.at("}") { Vec::new() } else { self.field_assigns()? };
        self.expect("}")?;
        let to = if self.eat("to") {
            let broker = self.ident()?;
            self.expect("topic")?;
            let topic = self.string()?;
            let key = if self.eat("key") { Some(self.expr()?) } else { None };
            Some(EmitTarget { broker, topic, key })
        } else {
            None
        };
        Ok(Emit { event, fields, to, span: s.to(self.prev_span()) })
    }

    fn subscribe(&mut self, start: Span) -> PResult<Decl> {
        self.expect("subscribe")?;
        let name = self.ident()?;
        let params = self.params()?;
        self.expect("{")?;
        let allow = self.allow()?;
        self.expect("from")?;
        let from = self.source_clause()?;
        let filter = if self.eat("where") { Some(self.expr()?) } else { None };
        self.expect("select")?;
        let select = self.selection()?;
        self.expect("}")?;
        Ok(Decl::Subscribe(SubscribeDecl { name, params, allow, from, filter, select, span: start.to(self.prev_span()) }))
    }

    /// `cross tenant` in front of a declaration that runs without an actor.
    fn cross_reaction(&mut self, start: Span) -> PResult<Decl> {
        Ok(match self.decl_after_modifier(start)? {
            Decl::Consume(d) => Decl::Consume(ConsumeDecl { cross_tenant: true, ..d }),
            Decl::OnEvent(d) => Decl::OnEvent(OnEventDecl { cross_tenant: true, ..d }),
            Decl::Schedule(d) => Decl::Schedule(ScheduleDecl { cross_tenant: true, ..d }),
            Decl::Rule(d) => Decl::Rule(RuleDecl { cross_tenant: true, ..d }),
            other => other,
        })
    }

    fn decl_after_modifier(&mut self, start: Span) -> PResult<Decl> {
        if self.at("consume") {
            self.consume(start)
        } else if self.at("on") {
            self.on_event(start)
        } else if self.at("schedule") {
            self.schedule(start)
        } else {
            self.rule(start)
        }
    }

    fn webhook(&mut self, start: Span) -> PResult<Decl> {
        self.expect("webhook")?;
        let name = self.ident()?;
        self.expect("via")?;
        let via = self.call_target()?;
        self.expect("{")?;
        let mut handlers = Vec::new();
        while !self.eat("}") {
            let cross_tenant = self.eat("cross");
            if cross_tenant {
                self.expect("tenant")?;
            }
            self.expect("on")?;
            let event_span = self.span();
            let event = if matches!(self.tok().tok, Tok::Str(_)) { self.string()? } else { self.ident()?.name };
            self.expect("(")?;
            let binding = self.ident()?;
            let ty = if self.eat(":") { Some(self.type_expr()?) } else { None };
            self.expect(")")?;
            self.expect("do")?;
            let body = self.block()?;
            handlers.push(WebhookOn { cross_tenant, event, event_span, binding, ty, body });
        }
        Ok(Decl::Webhook(WebhookDecl { name, via, handlers, span: start.to(self.prev_span()) }))
    }

    fn consume(&mut self, start: Span) -> PResult<Decl> {
        self.expect("consume")?;
        let broker = self.ident()?;
        self.expect("topic")?;
        let topic = self.string()?;
        self.expect("as")?;
        let name = self.ident()?;
        self.expect("{")?;
        self.expect("key")?;
        self.expect(":")?;
        let key = self.ident()?;
        let dedupe = if self.eat("dedupe") {
            self.expect("by")?;
            Some(self.ident()?)
        } else {
            None
        };
        self.expect("do")?;
        let body = self.block()?;
        self.expect("}")?;
        Ok(Decl::Consume(ConsumeDecl { cross_tenant: false, broker, topic, name, key, dedupe, body, span: start.to(self.prev_span()) }))
    }

    // ---------- statements ----------

    fn block(&mut self) -> PResult<Block> {
        let s = self.expect("{")?;
        let mut stmts = Vec::new();
        while !self.eat("}") {
            if self.at_eof() {
                return self.err("unclosed block");
            }
            stmts.push(self.stmt()?);
        }
        Ok(Block { stmts, span: s.to(self.prev_span()) })
    }

    fn field_assigns(&mut self) -> PResult<Vec<FieldAssign>> {
        let mut out = Vec::new();
        loop {
            if self.eat("...") {
                out.push(FieldAssign::Spread(self.expr()?));
            } else {
                let name = self.ident()?;
                let value = if self.eat(":") { Some(self.expr()?) } else { None };
                out.push(FieldAssign::Named { name, value });
            }
            if !self.eat(",") {
                break;
            }
        }
        Ok(out)
    }

    fn braced_fields(&mut self) -> PResult<Vec<FieldAssign>> {
        self.expect("{")?;
        let f = if self.at("}") { Vec::new() } else { self.field_assigns()? };
        self.expect("}")?;
        Ok(f)
    }

    fn assigns(&mut self) -> PResult<Vec<Assign>> {
        let mut out = Vec::new();
        loop {
            let s = self.span();
            let target = self.postfix()?;
            let op = if self.eat("=") {
                AssignOp::Set
            } else if self.eat("+=") {
                AssignOp::Add
            } else if self.eat("-=") {
                AssignOp::Sub
            } else {
                return self.err(format!("expected '=', '+=' or '-=' but found {}", Self::describe(self.tok())));
            };
            let value = self.expr()?;
            out.push(Assign { target, op, value, span: s.to(self.prev_span()) });
            if !self.eat(",") {
                break;
            }
        }
        Ok(out)
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        let start = self.span();
        let kw = match &self.tok().tok {
            Tok::Ident(s) => s.clone(),
            _ => return self.err(format!("expected a statement but found {}", Self::describe(self.tok()))),
        };
        let st = match kw.as_str() {
            "let" => Stmt::Let(self.let_clause()?),
            "insert" => {
                self.pos += 1;
                let entity = self.ident()?;
                let from = if self.eat("from") { Some(self.set_expr()?) } else { None };
                let fields = self.braced_fields()?;
                let bind = if self.eat("as") { Some(self.ident()?) } else { None };
                Stmt::Insert { entity, from, fields, bind, span: start.to(self.prev_span()) }
            }
            "upsert" => {
                self.pos += 1;
                let entity = self.ident()?;
                self.expect("by")?;
                self.expect("(")?;
                let mut keys = vec![self.ident()?];
                while self.eat(",") {
                    keys.push(self.ident()?);
                }
                self.expect(")")?;
                let fields = self.braced_fields()?;
                let bind = if self.eat("as") { Some(self.ident()?) } else { None };
                Stmt::Upsert { entity, keys, fields, bind, span: start.to(self.prev_span()) }
            }
            "update" => {
                self.pos += 1;
                let target = self.set_expr()?;
                let via = if self.eat("via") {
                    let path = self.postfix()?;
                    Some((path, self.ident()?))
                } else {
                    None
                };
                self.expect("set")?;
                let assigns = self.assigns()?;
                Stmt::Update { target, via, assigns, span: start.to(self.prev_span()) }
            }
            "delete" => {
                self.pos += 1;
                Stmt::Delete { target: self.set_expr()?, span: start.to(self.prev_span()) }
            }
            "purge" => {
                self.pos += 1;
                Stmt::Purge { target: self.set_expr()?, span: start.to(self.prev_span()) }
            }
            "erase" => {
                self.pos += 1;
                Stmt::Erase { target: self.expr()?, span: start.to(self.prev_span()) }
            }
            "toggle" => {
                self.pos += 1;
                let entity = self.ident()?;
                let fields = self.braced_fields()?;
                Stmt::Toggle { entity, fields, span: start.to(self.prev_span()) }
            }
            "set" => {
                self.pos += 1;
                Stmt::Set { assigns: self.assigns()?, span: start.to(self.prev_span()) }
            }
            "when" => {
                self.pos += 1;
                let cond = self.expr()?;
                let body = if self.eat(":") {
                    let s = self.span();
                    let inner = self.stmt()?;
                    Block { stmts: vec![inner], span: s.to(self.prev_span()) }
                } else {
                    self.block()?
                };
                Stmt::When { cond, body, span: start.to(self.prev_span()) }
            }
            "each" => {
                self.pos += 1;
                let source = self.set_expr()?;
                self.expect("partial").map_err(|d| {
                    d.with_help("'each' only exists as 'each <set> x partial { ... }' (per-item results); use set operations otherwise")
                })?;
                let body = self.block()?;
                Stmt::Each { source, body, span: start.to(self.prev_span()) }
            }
            "reserve" | "confirm" | "release" => {
                let op = self.ident()?;
                let what = self.ident()?;
                self.expect("of")?;
                let of = self.postfix()?;
                let duration = if self.eat("for") { Some(self.duration()?) } else { None };
                let code = if self.eat("else") { Some(self.code()?) } else { None };
                Stmt::Reserve { op, what, of, duration, code, span: start.to(self.prev_span()) }
            }
            "at" => {
                self.pos += 1;
                let at = self.expr()?;
                self.expect("run")?;
                let intent = self.ident()?;
                let args = self.args()?;
                Stmt::AtRun { at, intent, args, span: start.to(self.prev_span()) }
            }
            "notify" => Stmt::Notify(self.notify()?),
            "export" => {
                self.pos += 1;
                self.expect("personal")?;
                self.expect("data")?;
                self.expect("of")?;
                let of = self.expr()?;
                self.expect("to")?;
                let to = self.ident()?;
                let notify = if self.eat("notify") { Some(self.expr()?) } else { None };
                Stmt::ExportPersonalData { of, to, notify, span: start.to(self.prev_span()) }
            }
            _ => {
                let call = self.call_expr().map_err(|d| {
                    Diagnostic::error(codes::E100, format!("unknown statement starting with '{kw}'"), start).with_help(format!(
                        "statements: let insert upsert update delete purge erase toggle set when each reserve confirm release at notify export, or an extension effect call like s3.put(...) ({})",
                        d.message
                    ))
                })?;
                let bind = if self.eat("as") { Some(self.ident()?) } else { None };
                let into = if self.eat("into") { Some(self.postfix()?) } else { None };
                let on_failure = if self.at("on") && self.at_k(1, "failure") {
                    self.pos += 2;
                    self.expect("do")?;
                    Some(self.block()?)
                } else {
                    None
                };
                Stmt::Effect { call, bind, into, on_failure, span: start.to(self.prev_span()) }
            }
        };
        Ok(st)
    }

    fn notify(&mut self) -> PResult<Notify> {
        let s = self.expect("notify")?;
        let to = self.set_expr()?;
        self.expect("via")?;
        let via = self.call_target()?;
        let fields = if self.at("{") { self.braced_fields()? } else { Vec::new() };
        let category = if self.eat("respecting") {
            self.expect("preferences")?;
            self.expect("(")?;
            self.expect("category")?;
            self.expect(":")?;
            let c = self.ident()?;
            self.expect(")")?;
            Some(c)
        } else {
            None
        };
        let digest = if self.eat("digest") {
            self.expect("every")?;
            Some(self.duration()?)
        } else {
            None
        };
        Ok(Notify { to, via, fields, category, digest, span: s.to(self.prev_span()) })
    }

    // ---------- time, rules and L3 forms ----------

    fn on_event(&mut self, start: Span) -> PResult<Decl> {
        self.expect("on")?;
        let event = self.ident()?;
        let binding = self.ident()?;
        let when = if self.eat("when") { Some(self.expr()?) } else { None };
        let body = if self.at("notify") {
            let s = self.span();
            let n = self.notify()?;
            Block { stmts: vec![Stmt::Notify(n)], span: s.to(self.prev_span()) }
        } else {
            self.expect("do")?;
            self.block()?
        };
        Ok(Decl::OnEvent(OnEventDecl { cross_tenant: false, event, binding, when, body, span: start.to(self.prev_span()) }))
    }

    fn schedule(&mut self, start: Span) -> PResult<Decl> {
        self.expect("schedule")?;
        let name = self.ident()?;
        self.expect("every")?;
        let every = if self.eat("day") {
            Every::Day
        } else if self.eat("week") {
            self.expect("on")?;
            let d = self.ident()?;
            if !["mon", "tue", "wed", "thu", "fri", "sat", "sun"].contains(&d.name.as_str()) {
                return Err(Diagnostic::error(codes::E100, format!("'{}' is not a weekday", d.name), d.span)
                    .with_help("weekdays: mon tue wed thu fri sat sun"));
            }
            Every::Week(d)
        } else if self.eat("month") {
            self.expect("on")?;
            self.expect("day")?;
            Every::MonthDay(self.int()?)
        } else {
            Every::Interval(self.duration()?)
        };
        let at = if self.eat("at") {
            match self.tok().tok {
                Tok::TimeOfDay(h, m) => {
                    self.pos += 1;
                    Some((h, m))
                }
                _ => return self.err("expected a time of day like 00:00"),
            }
        } else {
            None
        };
        let tz = if self.eat("tz") { Some(self.string()?) } else { None };
        let catch_up_once = if self.eat("catch") {
            self.expect("up")?;
            if self.eat("once") {
                Some(true)
            } else {
                self.expect("skip")?;
                Some(false)
            }
        } else {
            None
        };
        self.expect("{")?;
        let for_each = if self.eat("for") { Some(self.set_expr()?) } else { None };
        let mut body = Vec::new();
        while !self.eat("}") {
            body.push(self.stmt()?);
        }
        Ok(Decl::Schedule(ScheduleDecl { cross_tenant: false, name, every, at, tz, catch_up_once, for_each, body, span: start.to(self.prev_span()) }))
    }

    fn retain(&mut self, start: Span) -> PResult<Decl> {
        self.expect("retain")?;
        let entity = self.ident()?;
        self.expect("for")?;
        let keep = self.duration()?;
        self.expect("after")?;
        let after = self.qualname()?;
        self.expect("then")?;
        let anonymize = if self.eat("anonymize") {
            true
        } else {
            self.expect("purge")?;
            false
        };
        let notify = if self.eat("notify") {
            let who = self.qualname()?;
            let before = self.duration()?;
            self.expect("before")?;
            self.expect("via")?;
            Some((who, before, self.call_target()?))
        } else {
            None
        };
        Ok(Decl::Retain(RetainDecl { entity, keep, after, anonymize, notify, span: start.to(self.prev_span()) }))
    }

    fn rule(&mut self, start: Span) -> PResult<Decl> {
        self.expect("rule")?;
        let name = self.ident()?;
        self.expect("on")?;
        let entity = self.ident()?;
        let alias = self.ident()?;
        self.expect("when")?;
        let when = self.expr()?;
        self.expect("do")?;
        let body = self.block()?;
        Ok(Decl::Rule(RuleDecl { cross_tenant: false, name, entity, alias, when, body, span: start.to(self.prev_span()) }))
    }

    fn projection(&mut self, start: Span) -> PResult<Decl> {
        self.expect("projection")?;
        let name = self.ident()?;
        self.expect("from")?;
        self.expect("events")?;
        let events = self.ident_list_bracket()?;
        self.expect("key")?;
        let key = self.ident()?;
        self.expect("{")?;
        let mut handlers = Vec::new();
        while !self.eat("}") {
            self.expect("on")?;
            let ev = self.ident()?;
            let bind = self.ident()?;
            self.expect(":")?;
            handlers.push((ev, bind, self.stmt()?));
        }
        Ok(Decl::Projection(ProjectionDecl { name, events, key, handlers, span: start.to(self.prev_span()) }))
    }

    fn search(&mut self, start: Span) -> PResult<Decl> {
        self.expect("search")?;
        let name = self.ident()?;
        self.expect("on")?;
        let entity = self.ident()?;
        self.expect("fields")?;
        self.expect("[")?;
        let mut fields = Vec::new();
        loop {
            let f = self.ident()?;
            let w = if self.eat("weight") { Some(self.ident()?) } else { None };
            fields.push((f, w));
            if !self.eat(",") {
                break;
            }
        }
        self.expect("]")?;
        let language = if self.eat("language") { Some(self.ident()?) } else { None };
        Ok(Decl::Search(SearchDecl { name, entity, fields, language, span: start.to(self.prev_span()) }))
    }

    fn job(&mut self, start: Span) -> PResult<Decl> {
        self.expect("job")?;
        let name = self.ident()?;
        let params = self.params()?;
        self.expect("{")?;
        let allow = self.allow()?;
        let progress = if self.eat("progress") {
            self.expect("over")?;
            Some(self.set_expr()?)
        } else {
            None
        };
        let produce = if self.eat("produce") {
            let fmt = self.ident()?;
            self.expect("to")?;
            let store = self.ident()?;
            self.expect("bucket")?;
            let bucket = self.string()?;
            let exp = if self.eat("expires") { Some(self.duration()?) } else { None };
            Some((fmt, store, bucket, exp))
        } else {
            None
        };
        let body = if self.eat("do") { Some(self.block()?) } else { None };
        let notify = if self.eat("notify") {
            let who = self.expr()?;
            self.expect("via")?;
            Some((who, self.call_target()?))
        } else {
            None
        };
        self.expect("}")?;
        Ok(Decl::Job(JobDecl { name, params, allow, progress, produce, body, notify, span: start.to(self.prev_span()) }))
    }

    fn verification(&mut self, start: Span) -> PResult<Decl> {
        self.expect("verification")?;
        let name = self.ident()?;
        self.expect("{")?;
        self.expect("subject")?;
        self.expect(":")?;
        let subject = self.ident()?;
        self.expect("target")?;
        self.expect(":")?;
        let target = self.type_expr()?;
        let target_where = if self.eat("where") { Some(self.expr()?) } else { None };
        self.expect("code")?;
        self.expect(":")?;
        let alnum = if self.eat("alnum") {
            true
        } else {
            self.expect("digits")?;
            false
        };
        let length = self.int()?;
        self.expect("ttl")?;
        let ttl = self.duration()?;
        self.expect("attempts")?;
        let attempts = self.int()?;
        let resend_after = if self.eat("resend") {
            self.expect("after")?;
            Some(self.duration()?)
        } else {
            None
        };
        self.expect("deliver")?;
        self.expect("via")?;
        let deliver = self.call_target()?;
        self.expect("on")?;
        self.expect("verified")?;
        self.expect("(")?;
        let a = self.ident()?;
        self.expect(",")?;
        let b = self.ident()?;
        self.expect(")")?;
        self.expect("do")?;
        let blk = self.block()?;
        self.expect("}")?;
        Ok(Decl::Verification(VerificationDecl {
            name,
            subject,
            target,
            target_where,
            alnum,
            length,
            ttl,
            attempts,
            resend_after,
            deliver,
            on_verified: (a, b, blk),
            span: start.to(self.prev_span()),
        }))
    }

    fn grant_link(&mut self, start: Span) -> PResult<Decl> {
        self.expect("grant")?;
        self.expect("link")?;
        let name = self.ident()?;
        self.expect("{")?;
        let grants = if self.eat("grants") {
            self.expect(":")?;
            let e = self.expr()?;
            self.expect("as")?;
            Some((e, self.ident()?))
        } else {
            None
        };
        self.expect("scope")?;
        self.expect(":")?;
        let mut scope = vec![self.param()?];
        while self.eat(",") {
            scope.push(self.param()?);
        }
        self.expect("issued")?;
        self.expect("by")?;
        let issued_by = self.expr()?;
        let to = if self.eat("to") { Some(self.expr()?) } else { None };
        self.expect("expires")?;
        let expires = self.duration()?;
        let uses = if self.eat("uses") { Some(self.int()?) } else { None };
        let redeem_params = if self.at("redeem") && self.at_k(1, "with") {
            self.pos += 2;
            self.params()?
        } else {
            Vec::new()
        };
        let mut requires = Vec::new();
        while self.at("require") {
            requires.push(self.require()?);
        }
        let on_redeem = if self.eat("on") {
            self.expect("redeem")?;
            self.expect("do")?;
            Some(self.block()?)
        } else {
            None
        };
        self.expect("}")?;
        if grants.is_none() && on_redeem.is_none() {
            return Err(Diagnostic::error(codes::E100, format!("grant link {} grants nothing", name.name), name.span)
                .with_help("add 'grants: <relation> as <ROLE>' or 'on redeem do { ... }'"));
        }
        Ok(Decl::GrantLink(GrantLinkDecl {
            name,
            grants,
            scope,
            redeem_params,
            issued_by,
            to,
            expires,
            uses,
            requires,
            on_redeem,
            span: start.to(self.prev_span()),
        }))
    }

    fn approval(&mut self, start: Span) -> PResult<Decl> {
        self.expect("approval")?;
        let name = self.ident()?;
        self.expect("for")?;
        let entity = self.ident()?;
        let alias = self.ident()?;
        self.expect("{")?;
        self.expect("approvers")?;
        self.expect(":")?;
        let approvers = self.set_expr()?;
        let requested_by = if self.eat("requested") {
            self.expect("by")?;
            Some(self.expr()?)
        } else {
            None
        };
        self.expect("require")?;
        let required = self.int()?;
        self.expect("approvals")?;
        let no_self = if self.eat(",") {
            self.expect("no")?;
            self.expect("self")?;
            self.expect("approval")?;
            true
        } else {
            false
        };
        self.expect("on")?;
        self.expect("approved")?;
        self.expect("do")?;
        let on_approved = self.block()?;
        self.expect("on")?;
        self.expect("rejected")?;
        self.expect("do")?;
        let on_rejected = self.block()?;
        let expires = if self.eat("expires") { Some(self.duration()?) } else { None };
        self.expect("}")?;
        Ok(Decl::Approval(ApprovalDecl {
            name,
            entity,
            alias,
            approvers,
            requested_by,
            required,
            no_self,
            on_approved,
            on_rejected,
            expires,
            span: start.to(self.prev_span()),
        }))
    }

    fn expose(&mut self, start: Span) -> PResult<Decl> {
        self.expect("expose")?;
        let entity = self.ident()?;
        self.expect("{")?;
        let mut d = ExposeDecl { entity, read: None, create: None, update: None, delete: None, span: start };
        while !self.eat("}") {
            let op = self.ident()?;
            self.expect(":")?;
            match op.name.as_str() {
                "read" => d.read = Some(if self.eat("visible") { None } else { Some(self.expr()?) }),
                "create" | "update" => {
                    let e = self.expr()?;
                    self.expect("fields")?;
                    let f = self.ident_list_bracket()?;
                    if op.name == "create" { d.create = Some((e, f)) } else { d.update = Some((e, f)) }
                }
                "delete" => d.delete = Some(self.expr()?),
                other => {
                    return Err(Diagnostic::error(codes::E100, format!("unknown expose operation '{other}'"), op.span)
                        .with_help("operations: read, create, update, delete"));
                }
            }
        }
        d.span = start.to(self.prev_span());
        Ok(Decl::Expose(d))
    }

    fn outbound(&mut self, start: Span) -> PResult<Decl> {
        self.expect("outbound")?;
        self.expect("webhooks")?;
        self.expect("for")?;
        let entity = self.ident()?;
        let alias = self.ident()?;
        self.expect("{")?;
        self.expect("events")?;
        let events = self.ident_list_bracket()?;
        let filter = if self.eat("where") { Some(self.expr()?) } else { None };
        self.expect("sign")?;
        let sign = self.ident()?;
        self.expect("retry")?;
        let retry = self.int()?;
        self.expect("over")?;
        let over = self.duration()?;
        let disable_after = if self.eat("disable") {
            self.expect("after")?;
            let d = self.duration()?;
            self.expect("failing")?;
            Some(d)
        } else {
            None
        };
        self.expect("}")?;
        Ok(Decl::OutboundWebhooks(OutboundDecl { entity, alias, events, filter, sign, retry, over, disable_after, span: start.to(self.prev_span()) }))
    }

    fn consent(&mut self, start: Span) -> PResult<Decl> {
        self.expect("consent")?;
        let name = self.ident()?;
        self.expect("version")?;
        let version = self.int()?;
        self.expect("required")?;
        self.expect("for")?;
        let intents = self.ident_list_bracket()?;
        Ok(Decl::Consent(ConsentDecl { name, version, intents, span: start.to(self.prev_span()) }))
    }

    fn config(&mut self, start: Span) -> PResult<Decl> {
        self.expect("config")?;
        let name = self.ident()?;
        self.expect(":")?;
        let ty = self.type_expr()?;
        let default = if self.eat("=") { Some(self.expr()?) } else { None };
        Ok(Decl::Config(ConfigDecl { name, ty, default, span: start.to(self.prev_span()) }))
    }

    fn flag(&mut self, start: Span) -> PResult<Decl> {
        self.expect("flag")?;
        let name = self.ident()?;
        self.expect("default")?;
        let default_on = if self.eat("on") {
            true
        } else {
            self.expect("off")?;
            false
        };
        let rollout = if self.eat("rollout") {
            let n = self.int()?;
            self.expect("%")?;
            self.expect("by")?;
            self.expect("actor")?;
            Some(n)
        } else {
            None
        };
        Ok(Decl::Flag(FlagDecl { name, default_on, rollout, span: start.to(self.prev_span()) }))
    }

    fn impersonate(&mut self, start: Span) -> PResult<Decl> {
        self.expect("impersonate")?;
        let entity = self.ident()?;
        self.expect("by")?;
        let by = self.expr()?;
        self.expect("audited")?;
        self.expect("reason")?;
        self.expect("required")?;
        self.expect("ttl")?;
        let ttl = self.duration()?;
        Ok(Decl::Impersonate(ImpersonateDecl { entity, by, ttl, span: start.to(self.prev_span()) }))
    }

    // ---------- expressions ----------

    pub fn expr(&mut self) -> PResult<Expr> {
        self.or()
    }

    fn bin(l: Expr, op: BinOp, r: Expr) -> Expr {
        let span = l.span.to(r.span);
        Expr { kind: ExprKind::Binary(op, Box::new(l), Box::new(r)), span }
    }

    fn or(&mut self) -> PResult<Expr> {
        let mut l = self.and()?;
        while self.eat("or") {
            let r = self.and()?;
            l = Self::bin(l, BinOp::Or, r);
        }
        Ok(l)
    }

    fn and(&mut self) -> PResult<Expr> {
        let mut l = self.not()?;
        while self.eat("and") {
            let r = self.not()?;
            l = Self::bin(l, BinOp::And, r);
        }
        Ok(l)
    }

    fn not(&mut self) -> PResult<Expr> {
        if self.at("not") {
            let s = self.span();
            self.pos += 1;
            let e = self.not()?;
            let span = s.to(e.span);
            return Ok(Expr { kind: ExprKind::Not(Box::new(e)), span });
        }
        self.cmp()
    }

    fn cmp(&mut self) -> PResult<Expr> {
        let l = self.add()?;
        let op = match &self.tok().tok {
            Tok::Punct("=") => Some(BinOp::Eq),
            Tok::Punct("!=") => Some(BinOp::Ne),
            Tok::Punct("<") => Some(BinOp::Lt),
            Tok::Punct("<=") => Some(BinOp::Le),
            Tok::Punct(">") => Some(BinOp::Gt),
            Tok::Punct(">=") => Some(BinOp::Ge),
            _ => None,
        };
        if let Some(op) = op {
            self.pos += 1;
            let r = self.add()?;
            return Ok(Self::bin(l, op, r));
        }
        if self.eat("in") {
            if self.at("[") {
                self.pos += 1;
                let mut items = Vec::new();
                if !self.at("]") {
                    items.push(self.expr()?);
                    if self.eat(",") {
                        let second = self.expr()?;
                        if self.at(")") {
                            self.pos += 1;
                            let span = l.span.to(self.prev_span());
                            let first = items.pop().expect("one item parsed");
                            return Ok(Expr { kind: ExprKind::InRange(Box::new(l), Box::new(first), Box::new(second)), span });
                        }
                        items.push(second);
                        while self.eat(",") {
                            items.push(self.expr()?);
                        }
                    }
                }
                self.expect("]")?;
                let span = l.span.to(self.prev_span());
                return Ok(Expr { kind: ExprKind::InList(Box::new(l), items), span });
            }
            let r = self.add()?;
            let span = l.span.to(r.span);
            return Ok(Expr { kind: ExprKind::InExpr(Box::new(l), Box::new(r)), span });
        }
        if self.eat("is") {
            let name = self.ident()?;
            let span = l.span.to(name.span);
            return Ok(Expr { kind: ExprKind::Is(Box::new(l), name), span });
        }
        if self.at("has") && self.at_k(1, "scope") {
            self.pos += 2;
            let q = self.qualname()?;
            let span = l.span.to(q.span);
            return Ok(Expr { kind: ExprKind::HasScope(Box::new(l), q), span });
        }
        Ok(l)
    }

    fn add(&mut self) -> PResult<Expr> {
        let mut l = self.mul()?;
        loop {
            let op = if self.at("+") {
                BinOp::Add
            } else if self.at("-") && !(self.at_k(1, "class")) {
                BinOp::Sub
            } else {
                break;
            };
            self.pos += 1;
            let r = self.mul()?;
            l = Self::bin(l, op, r);
        }
        Ok(l)
    }

    fn mul(&mut self) -> PResult<Expr> {
        let mut l = self.unary()?;
        loop {
            let op = if self.at("*") {
                BinOp::Mul
            } else if self.at("/") {
                BinOp::Div
            } else {
                break;
            };
            self.pos += 1;
            let r = self.unary()?;
            l = Self::bin(l, op, r);
        }
        Ok(l)
    }

    fn unary(&mut self) -> PResult<Expr> {
        if self.at("-") {
            let s = self.span();
            self.pos += 1;
            let e = self.unary()?;
            let span = s.to(e.span);
            return Ok(Expr { kind: ExprKind::Neg(Box::new(e)), span });
        }
        self.postfix()
    }

    fn postfix(&mut self) -> PResult<Expr> {
        let mut e = self.primary()?;
        while self.at(".") && matches!(self.peek_tok(1).tok, Tok::Ident(_)) {
            self.pos += 1;
            let f = self.ident()?;
            if self.at("(") {
                // method-style call on a computed value is not in the grammar
                return Err(Diagnostic::error(codes::E100, format!("cannot call '.{}(...)' on an expression", f.name), f.span)
                    .with_help("calls are written as qualified names: ext.fn(args) or relation(args)"));
            }
            let span = e.span.to(f.span);
            e = Expr { kind: ExprKind::Field(Box::new(e), f), span };
        }
        Ok(e)
    }

    fn args(&mut self) -> PResult<Vec<Arg>> {
        self.expect("(")?;
        let mut out = Vec::new();
        if !self.at(")") {
            loop {
                let name = if self.is_ident() && self.at_k(1, ":") {
                    let n = self.ident()?;
                    self.pos += 1;
                    Some(n)
                } else {
                    None
                };
                let mut value = self.expr()?;
                // binder argument: `same(items x: x.club)`
                if self.is_ident() && self.at_k(1, ":") && self.tok().span.line == self.prev_span().line {
                    let alias = self.ident()?;
                    self.pos += 1;
                    let body = self.expr()?;
                    let span = value.span.to(body.span);
                    let set = SetExpr { span: value.span.to(alias.span), source: value, alias: Some(alias), filter: None };
                    value = Expr { kind: ExprKind::Binder(Box::new(set), Box::new(body)), span };
                }
                out.push(Arg { name, value });
                if !self.eat(",") {
                    break;
                }
            }
        }
        self.expect(")")?;
        Ok(out)
    }

    fn call_expr(&mut self) -> PResult<CallExpr> {
        let callee = self.qualname()?;
        if !self.at("(") {
            return Err(Diagnostic::error(codes::E100, format!("expected '(' after '{}'", callee.text()), self.span()));
        }
        let args = self.args()?;
        let span = callee.span.to(self.prev_span());
        Ok(CallExpr { callee, args, span })
    }

    /// Alias rule: same line as the preceding token, not a clause word, and not
    /// the start of a `name:` item.
    fn opt_alias(&mut self) -> Option<Ident> {
        self.opt_alias_ext(false)
    }

    fn opt_alias_ext(&mut self, binder: bool) -> Option<Ident> {
        let Tok::Ident(name) = &self.tok().tok else { return None };
        if STOP_WORDS.contains(&name.as_str()) || (!binder && self.at_k(1, ":")) || self.tok().span.line != self.prev_span().line {
            return None;
        }
        let id = Ident { name: name.clone(), span: self.span() };
        self.pos += 1;
        Some(id)
    }

    pub fn set_expr(&mut self) -> PResult<SetExpr> {
        self.set_expr_ext(false)
    }

    /// Set expression inside an aggregate/quantifier, where `alias:` introduces the body.
    fn binder_set_expr(&mut self) -> PResult<SetExpr> {
        self.set_expr_ext(true)
    }

    fn set_expr_ext(&mut self, binder: bool) -> PResult<SetExpr> {
        let s = self.span();
        let source = self.postfix()?;
        let alias = self.opt_alias_ext(binder);
        let filter = if self.eat("where") { Some(Box::new(self.expr()?)) } else { None };
        Ok(SetExpr { source, alias, filter, span: s.to(self.prev_span()) })
    }

    fn primary(&mut self) -> PResult<Expr> {
        let s = self.span();
        let t = self.tok().tok.clone();
        let lit = |kind: ExprKind, p: &mut Self| {
            p.pos += 1;
            Ok(Expr { kind, span: s })
        };
        match t {
            Tok::Int(n) => lit(ExprKind::Int(n), self),
            Tok::Decimal(d) => lit(ExprKind::Decimal(d), self),
            Tok::Str(v) => lit(ExprKind::Str(v), self),
            Tok::Duration(n, unit) => lit(ExprKind::Duration(Duration { n, unit }), self),
            Tok::Size(n) => lit(ExprKind::Size(n), self),
            Tok::TimeOfDay(h, m) => lit(ExprKind::TimeOfDay(h, m), self),
            Tok::Punct("(") => {
                self.pos += 1;
                let e = self.expr()?;
                self.expect(")")?;
                Ok(Expr { kind: e.kind, span: s.to(self.prev_span()) })
            }
            Tok::Punct("[") => {
                self.pos += 1;
                let mut items = Vec::new();
                if !self.at("]") {
                    loop {
                        items.push(self.expr()?);
                        if !self.eat(",") {
                            break;
                        }
                    }
                }
                self.expect("]")?;
                Ok(Expr { kind: ExprKind::List(items), span: s.to(self.prev_span()) })
            }
            Tok::Ident(w) => self.word(w, s),
            _ => self.err(format!("expected an expression but found {}", Self::describe(self.tok()))),
        }
    }

    fn word(&mut self, w: String, s: Span) -> PResult<Expr> {
        let kwk = |k: Kw, p: &mut Self| {
            p.pos += 1;
            Ok(Expr { kind: ExprKind::Kw(k), span: s })
        };
        match w.as_str() {
            "true" => {
                self.pos += 1;
                Ok(Expr { kind: ExprKind::Bool(true), span: s })
            }
            "false" => {
                self.pos += 1;
                Ok(Expr { kind: ExprKind::Bool(false), span: s })
            }
            "null" => {
                self.pos += 1;
                Ok(Expr { kind: ExprKind::Null, span: s })
            }
            "actor" => kwk(Kw::Actor, self),
            "self" => kwk(Kw::SelfRow, self),
            "this" => kwk(Kw::This, self),
            "now" => kwk(Kw::Now, self),
            "today" => kwk(Kw::Today, self),
            "public" => kwk(Kw::Public, self),
            "authenticated" => kwk(Kw::Authenticated, self),
            "exists" => {
                self.pos += 1;
                if self.at("(") {
                    return self
                        .err::<Expr>("'exists' takes a set expression without parentheses")
                        .map_err(|d| d.with_help("write 'exists Entity x where ...' or 'exists relation(a, b)'"));
                }
                let se = self.set_expr()?;
                Ok(Expr { span: s.to(se.span), kind: ExprKind::Exists(Box::new(se)) })
            }
            "the" => {
                self.pos += 1;
                let se = self.set_expr()?;
                Ok(Expr { span: s.to(se.span), kind: ExprKind::The(Box::new(se)) })
            }
            "latest" => {
                self.pos += 1;
                let se = self.set_expr()?;
                self.expect("by").map_err(|d| d.with_help("latest needs an ordering: latest X x where ... by x.createdAt"))?;
                let by = self.expr()?;
                Ok(Expr { span: s.to(by.span), kind: ExprKind::Latest(Box::new(se), Box::new(by)) })
            }
            "first" => {
                self.pos += 1;
                let se = self.set_expr()?;
                self.expect("order")?;
                self.expect("by")?;
                let keys = self.sort_keys()?;
                Ok(Expr { span: s.to(self.prev_span()), kind: ExprKind::First(Box::new(se), keys) })
            }
            "if" => {
                self.pos += 1;
                let c = self.expr()?;
                self.expect("then")?;
                let t = self.expr()?;
                self.expect("else")?;
                let e = self.expr()?;
                Ok(Expr { span: s.to(e.span), kind: ExprKind::If(Box::new(c), Box::new(t), Box::new(e)) })
            }
            "count" | "sum" | "min" | "max" | "avg" if self.at_k(1, "(") => {
                let func = match w.as_str() {
                    "count" => AggFn::Count,
                    "sum" => AggFn::Sum,
                    "min" => AggFn::Min,
                    "max" => AggFn::Max,
                    _ => AggFn::Avg,
                };
                self.pos += 2;
                if self.eat(")") {
                    if func != AggFn::Count {
                        return self.err("only count() may be written without arguments (group-by context)");
                    }
                    return Ok(Expr { span: s.to(self.prev_span()), kind: ExprKind::Agg { func, set: None, body: None } });
                }
                let se = self.binder_set_expr()?;
                let body = if self.eat(":") { Some(Box::new(self.expr()?)) } else { None };
                self.expect(")")?;
                Ok(Expr { span: s.to(self.prev_span()), kind: ExprKind::Agg { func, set: Some(Box::new(se)), body } })
            }
            "all" | "any" if self.at_k(1, "(") => {
                self.pos += 2;
                let se = self.binder_set_expr()?;
                self.expect(":").map_err(|d| d.with_help("write all(items i: <condition on i>)"))?;
                let body = self.expr()?;
                self.expect(")")?;
                Ok(Expr { span: s.to(self.prev_span()), kind: ExprKind::Quant { all: w == "all", set: Box::new(se), body: Box::new(body) } })
            }
            "running_sum" => {
                self.pos += 1;
                self.expect("(")?;
                let value = self.expr()?;
                self.expect(")")?;
                self.expect("over")?;
                let over = self.ident()?;
                self.expect("order")?;
                self.expect("by")?;
                let order = self.add()?;
                Ok(Expr { span: s.to(order.span), kind: ExprKind::RunningSum { value: Box::new(value), over, order: Box::new(order) } })
            }
            _ => {
                let name = self.qualname()?;
                if self.at("(") {
                    let args = self.args()?;
                    let span = name.span.to(self.prev_span());
                    return Ok(Expr { kind: ExprKind::Call(CallExpr { callee: name, args, span }), span });
                }
                let mut parts = name.parts.into_iter();
                let first = parts.next().expect("qualname has a part");
                let mut e = Expr { span: first.span, kind: ExprKind::Name(first) };
                for p in parts {
                    let span = e.span.to(p.span);
                    e = Expr { kind: ExprKind::Field(Box::new(e), p), span };
                }
                Ok(e)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok(src: &str) -> File {
        match parse_file(src) {
            Ok(f) => f,
            Err(d) => panic!("{}", d.render("t.aip")),
        }
    }

    fn fails(src: &str) -> Diagnostic {
        parse_file(src).expect_err("should fail")
    }

    #[test]
    fn entity_with_mods_and_constraints() {
        let f = ok(r#"
entity ClubMember {
  club: Club
  member: Member?     on erase anonymize
  role: ClubRole
  email: Email personal encrypted visible to self or actor.role = ADMIN
               masked unless self as mask.email
  name: Text(1..30, trim)
  unique (club, member) else ALREADY_CLUB_MEMBER
  exactly one where role = ADMIN per club
  lifecycle status {
    PENDING            -> INTERVIEW
    PENDING, INTERVIEW -> APPROVED, REFUSED
  }
  visible to actor when exists membership(actor, club)
}"#);
        let Decl::Entity(e) = &f.decls[0] else { panic!() };
        assert_eq!(e.members.len(), 9);
        let EntityMember::Field(email) = &e.members[3] else { panic!() };
        assert_eq!(email.mods.len(), 4);
        let EntityMember::Lifecycle(l) = &e.members[7] else { panic!() };
        assert_eq!(l.transitions.len(), 2);
    }

    #[test]
    fn alias_does_not_cross_lines() {
        let f = ok(r#"
command SubmitApply(recruitment: Recruitment) {
  allow authenticated and not exists membership(actor, recruitment.club)
  require recruitment is open else RECRUITMENT_CLOSED
  do { insert Apply { member: actor, recruitment } as apply }
}"#);
        let Decl::Command(c) = &f.decls[0] else { panic!() };
        assert_eq!(c.requires.len(), 1);
    }

    #[test]
    fn clause_order_is_enforced() {
        let d = fails("command C { require x else A allow public }");
        assert!(d.message.contains("'allow' must come before 'require'"), "{}", d.message);
    }

    #[test]
    fn set_expr_aggregates_and_quantifiers() {
        ok(r#"
command C(targets: Set<ClubMember> max 200) {
  let club = same(targets t: t.club) else DIFFERENT
  allow managerOf(actor, club) and all(targets t: outranks(actor, t))
  do {
    update Product p via order.items i set p.stock += sum(i.quantity)
    set order.total = sum(order.items i: i.price * i.quantity) - discount(coupon)
  }
}"#);
    }

    #[test]
    fn half_open_range_and_list() {
        let f = ok("relation r(x: Time) = x in [today + 1d, today + 2d) or x in [A, B]");
        let Decl::Relation(r) = &f.decls[0] else { panic!() };
        let ExprKind::Binary(BinOp::Or, l, rr) = &r.body.kind else { panic!() };
        assert!(matches!(l.kind, ExprKind::InRange(..)));
        assert!(matches!(rr.kind, ExprKind::InList(..)));
    }

    #[test]
    fn unbounded_set_is_a_syntax_error() {
        let d = fails("command C(xs: Set<Apply>) { allow public }");
        assert!(d.message.contains("upper bound"));
    }

    #[test]
    fn selection_items_do_not_become_aliases() {
        ok(r#"
query Q(r: Recruitment) cached 30s per actor-class {
  allow public
  from r
  select {
    id title
    club { id name }
    isMyClub: exists membership(actor, r.club) isMyApply: exists Apply a where a.recruitment = r
    draft: (latest ApplyDraft d where d.member = actor by d.createdAt).id
  }
  touch r.views
}"#);
    }

    #[test]
    fn effect_statement_with_failure_path() {
        ok(r#"
command CancelOrder(order: Order) idempotent by order {
  allow order.customer = actor
  do {
    payments.refund(order.paymentRef, order.total, key: order.id) on failure do {
      set order.refundStatus = FAILED
    }
    when file: s3.put(file, bucket: "apply") into apply.file
    set order.status = CANCELLED
  }
  emit OrderCancelled { order } to kafka topic "orders" key order.id
}"#);
    }
}
