use crate::ast::*;
use crate::diag::{Diag, Span};
use crate::lexer::{lex, Tok, Token};
use crate::limits::{MAX_AST_DEPTH, MAX_EXPRESSION_DEPTH, MAX_EXPR_NODES};
use std::collections::BTreeSet;

/// 블록 안 같은 키를 두 번 쓰면 거부한다. 뒤 값이 앞 값을 조용히 덮어쓰지 않게 한다(F09).
fn once(seen: &mut BTreeSet<String>, key: &str, sp: Span, what: &str) -> Result<(), Diag> {
    if seen.insert(key.to_string()) {
        Ok(())
    } else {
        Err(Diag::new("DUPLICATE", format!("{what} 안 `{key}` 중복"), sp))
    }
}

pub struct Parser {
    toks: Vec<Token>,
    pos: usize,
    expression_depth: usize,
    expression_nodes: usize,
}

type R<T> = Result<T, Diag>;

pub fn parse_spec(src: &str, line_base: u32, col_base: u32) -> R<Spec> {
    let mut p = Parser::from_str(src, Span { line: line_base, col: col_base })?;
    p.spec()
}

/// H 형식의 정책 문자열을 A와 같은 식 문법으로 읽는다. 문자열 전체가 식 하나여야 한다.
pub fn parse_expr_str(src: &str, span: Span) -> R<Expr> {
    let mut p = Parser::from_str(src, span)?;
    let e = p.expr()?;
    p.expect_eof()?;
    Ok(e)
}

pub fn parse_type_str(src: &str, span: Span) -> R<TypeRef> {
    let mut p = Parser::from_str(src, span)?;
    let t = p.type_ref()?;
    p.expect_eof()?;
    Ok(t)
}

pub fn parse_duration_str(src: &str, span: Span) -> R<u64> {
    let mut p = Parser::from_str(src, span)?;
    let t = p.next();
    p.expect_eof()?;
    match t.tok {
        Tok::Dur(ms) => Ok(ms),
        _ => Err(Diag::new("PARSE_EXPECTED", "시간 값(예: 2s) 필요", span)),
    }
}

impl Parser {
    pub fn from_str(src: &str, span: Span) -> R<Parser> {
        Ok(Parser { toks: lex(src, span.line, span.col)?, pos: 0, expression_depth: 0, expression_nodes: 0 })
    }

    fn peek(&self) -> &Tok {
        &self.toks[self.pos].tok
    }
    fn span(&self) -> Span {
        self.toks[self.pos].span
    }
    fn next(&mut self) -> Token {
        let t = self.toks[self.pos].clone();
        if self.pos + 1 < self.toks.len() {
            self.pos += 1;
        }
        t
    }
    fn is_kw(&self, kw: &str) -> bool {
        matches!(self.peek(), Tok::Ident(s) if s == kw)
    }
    fn is_sym(&self, s: &str) -> bool {
        matches!(self.peek(), Tok::Sym(x) if *x == s)
    }
    fn eat_sym(&mut self, s: &str) -> bool {
        if self.is_sym(s) {
            self.next();
            true
        } else {
            false
        }
    }
    fn err<T>(&self, what: &str) -> R<T> {
        Err(Diag::new("PARSE_EXPECTED", format!("{what} 필요, `{:?}` 발견", self.peek()), self.span()))
    }
    fn expect_sym(&mut self, s: &str) -> R<()> {
        if self.eat_sym(s) {
            Ok(())
        } else {
            self.err(&format!("`{s}`"))
        }
    }
    fn expect_kw(&mut self, kw: &str) -> R<()> {
        if self.is_kw(kw) {
            self.next();
            Ok(())
        } else {
            self.err(&format!("`{kw}`"))
        }
    }
    pub fn expect_eof(&mut self) -> R<()> {
        if *self.peek() == Tok::Eof {
            Ok(())
        } else {
            self.err("입력 끝")
        }
    }
    fn ident(&mut self) -> R<String> {
        match self.peek().clone() {
            Tok::Ident(s) => {
                self.next();
                Ok(s)
            }
            _ => self.err("이름"),
        }
    }
    fn int(&mut self) -> R<i64> {
        match *self.peek() {
            Tok::Int(n) => {
                self.next();
                Ok(n)
            }
            _ => self.err("정수"),
        }
    }
    fn dur(&mut self) -> R<u64> {
        match *self.peek() {
            Tok::Dur(n) => {
                self.next();
                Ok(n)
            }
            _ => self.err("시간 값"),
        }
    }
    fn string(&mut self) -> R<String> {
        match self.peek().clone() {
            Tok::Str(s) => {
                self.next();
                Ok(s)
            }
            _ => self.err("문자열"),
        }
    }
    fn semis(&mut self) {
        while self.eat_sym(";") {}
    }
    fn ident_list(&mut self) -> R<Vec<(String, Span)>> {
        let mut v = vec![];
        loop {
            let sp = self.span();
            v.push((self.ident()?, sp));
            if !self.eat_sym(",") {
                return Ok(v);
            }
        }
    }

    fn spec(&mut self) -> R<Spec> {
        let mut spec = Spec::default();
        loop {
            self.semis();
            let sp = self.span();
            match self.peek().clone() {
                Tok::Eof => return Ok(spec),
                Tok::Ident(kw) => {
                    self.next();
                    match kw.as_str() {
                        "enum" => {
                            let name = self.ident()?;
                            self.expect_sym("{")?;
                            let variants = self.ident_list()?.into_iter().map(|v| v.0).collect();
                            self.expect_sym("}")?;
                            spec.enums.push(EnumDecl { name, variants, span: sp });
                        }
                        "actor" => spec.actor = Some((self.ident()?, sp)),
                        "predicate" => {
                            let name = self.ident()?;
                            let params = self.params_paren()?;
                            self.expect_sym("=")?;
                            let body = self.expr()?;
                            spec.predicates.push(Predicate { name, params, body, span: sp });
                        }
                        "access" => {
                            let name = self.ident()?;
                            let params = if self.is_sym("(") { self.params_paren()? } else { vec![] };
                            self.expect_sym("=")?;
                            let body = if self.is_kw("totalOfVisible") {
                                self.next();
                                self.expect_sym("(")?;
                                let bsp = self.span();
                                let r = self.ident()?;
                                self.expect_sym(")")?;
                                AccessBody::TotalOfVisible(r, bsp)
                            } else {
                                AccessBody::Guard(self.expr()?)
                            };
                            spec.accesses.push(Access { name, params, body, span: sp });
                        }
                        "limit" => {
                            let name = self.ident()?;
                            self.expect_kw("on")?;
                            let on = self.ident()?;
                            self.expect_sym("=")?;
                            self.expect_kw("atMost")?;
                            let at_most = self.int()?;
                            self.expect_kw("where")?;
                            let cond = self.expr()?;
                            spec.limits.push(Limit { name, on, at_most, cond, span: sp });
                        }
                        "resource" => spec.resources.push(self.resource(sp)?),
                        other => return Err(Diag::new("PARSE_UNKNOWN_DECL", format!("알 수 없는 선언 `{other}`"), sp)),
                    }
                }
                _ => return self.err("선언"),
            }
        }
    }

    fn params_paren(&mut self) -> R<Params> {
        self.expect_sym("(")?;
        let mut v = vec![];
        if !self.is_sym(")") {
            loop {
                let n = self.ident()?;
                self.expect_sym(":")?;
                v.push((n, self.type_ref()?));
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }

    fn params_block(&mut self) -> R<Params> {
        self.expect_sym("{")?;
        let mut v = vec![];
        loop {
            self.semis();
            if self.eat_sym("}") {
                return Ok(v);
            }
            let n = self.ident()?;
            self.expect_sym(":")?;
            v.push((n, self.type_ref()?));
        }
    }

    pub fn type_ref(&mut self) -> R<TypeRef> {
        let span = self.span();
        let name = self.ident()?;
        let mut t = TypeRef { name, id_of: false, nullable: false, range: None, span };
        if self.eat_sym(".") {
            self.expect_kw("Id")?;
            t.id_of = true;
        }
        if self.eat_sym("(") {
            let lo = self.int()?;
            self.expect_sym("..")?;
            let hi = self.int()?;
            self.expect_sym(")")?;
            t.range = Some((lo, hi));
        }
        if self.eat_sym("?") {
            t.nullable = true;
        }
        Ok(t)
    }

    fn resource(&mut self, span: Span) -> R<Resource> {
        let mut r = Resource { name: self.ident()?, span, ..Default::default() };
        let mut seen = BTreeSet::new();
        self.expect_sym("{")?;
        loop {
            self.semis();
            if self.eat_sym("}") {
                return Ok(r);
            }
            let sp = self.span();
            let kw = self.ident()?;
            if matches!(kw.as_str(), "fields" | "docs") {
                once(&mut seen, &kw, sp, "resource")?;
            }
            match kw.as_str() {
                "fields" => {
                    self.expect_sym("{")?;
                    loop {
                        self.semis();
                        if self.eat_sym("}") {
                            break;
                        }
                        let fsp = self.span();
                        let name = self.ident()?;
                        self.expect_sym(":")?;
                        r.fields.push(Field { name, ty: self.type_ref()?, span: fsp });
                    }
                }
                "rows" => {
                    self.expect_kw("read")?;
                    self.expect_kw("when")?;
                    if r.row_read.is_some() {
                        return Err(Diag::new("DUPLICATE", "rows read 정책 중복", sp));
                    }
                    r.row_read = Some(self.expr()?);
                }
                "field" => {
                    let f = self.ident()?;
                    self.expect_kw("read")?;
                    self.expect_kw("when")?;
                    r.field_read.push((f, self.expr()?, sp));
                }
                "expose" => {
                    if self.is_kw("apply") {
                        self.next();
                        let transition = self.ident()?;
                        let mut ea = ExposeApply { transition, targets: vec![], bulk: None, same_scope: None, span: sp };
                        let mut aseen = BTreeSet::new();
                        self.expect_sym("{")?;
                        loop {
                            self.semis();
                            if self.eat_sym("}") {
                                break;
                            }
                            let ksp = self.span();
                            let key = self.ident()?;
                            once(&mut aseen, &key, ksp, "expose apply")?;
                            match key.as_str() {
                                "target" => ea.targets = self.ident_list()?,
                                "bulk" => {
                                    self.expect_kw("maxRows")?;
                                    ea.bulk = Some(self.int()?);
                                }
                                "sameScope" => ea.same_scope = Some(self.unary()?),
                                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("expose apply 안 알 수 없는 키 `{o}`"), ksp)),
                            }
                        }
                        r.expose_apply.push(ea);
                    } else if self.is_kw("create") {
                        self.next();
                        if r.expose_create.is_some() {
                            return Err(Diag::new("DUPLICATE", "expose create 중복", sp));
                        }
                        let mut ec = ExposeCreate { allow: None, fields: vec![], span: sp };
                        let mut cseen = BTreeSet::new();
                        self.expect_sym("{")?;
                        loop {
                            self.semis();
                            if self.eat_sym("}") {
                                break;
                            }
                            let ksp = self.span();
                            let key = self.ident()?;
                            once(&mut cseen, &key, ksp, "expose create")?;
                            match key.as_str() {
                                "allow" => ec.allow = Some(self.expr()?),
                                "fields" => ec.fields = self.ident_list()?,
                                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("expose create 안 알 수 없는 키 `{o}`"), ksp)),
                            }
                        }
                        r.expose_create = Some(ec);
                    } else if self.is_kw("compose") {
                        self.next();
                        if r.expose_compose.is_some() {
                            return Err(Diag::new("DUPLICATE", "expose compose 중복", sp));
                        }
                        let mut ec = ExposeCompose { bulk: None, same_scope: None, transitions: vec![], creates: vec![], self_row: None, span: sp };
                        let mut cseen = BTreeSet::new();
                        self.expect_sym("{")?;
                        loop {
                            self.semis();
                            if self.eat_sym("}") {
                                break;
                            }
                            let ksp = self.span();
                            let key = self.ident()?;
                            if key != "create" {
                                once(&mut cseen, &key, ksp, "expose compose")?;
                            }
                            match key.as_str() {
                                "bulk" => {
                                    self.expect_kw("maxRows")?;
                                    ec.bulk = Some(self.int()?);
                                }
                                "sameScope" => ec.same_scope = Some(self.unary()?),
                                "transitions" => ec.transitions = self.ident_list()?,
                                "selfRow" => {
                                    let a = self.ident()?;
                                    self.expect_kw("by")?;
                                    ec.self_row = Some((a, self.ident()?, ksp));
                                }
                                "create" => {
                                    let res = self.ident()?;
                                    self.expect_kw("from")?;
                                    let mut srcs = vec![self.unary()?];
                                    while self.eat_sym(",") {
                                        srcs.push(self.unary()?);
                                    }
                                    ec.creates.push((res, srcs, ksp));
                                }
                                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("expose compose 안 알 수 없는 키 `{o}`"), ksp)),
                            }
                        }
                        r.expose_compose = Some(ec);
                    } else if self.is_kw("aggregate") {
                        self.next();
                        let asp = self.span();
                        r.expose_aggregates.push((self.ident()?, asp));
                    } else {
                        self.expect_kw("read")?;
                        if r.expose_read.is_some() {
                            return Err(Diag::new("DUPLICATE", "expose read 중복", sp));
                        }
                        r.expose_read = Some(self.expose_read(sp)?);
                    }
                }
                "aggregate" => r.aggregates.push(self.aggregate(sp)?),
                "transition" => {
                    let name = self.ident()?;
                    self.expect_sym("{")?;
                    let (mut from, mut to, mut allow, mut repeat) = (None, vec![], None, None);
                    let mut effects = vec![];
                    let mut tseen = BTreeSet::new();
                    loop {
                        self.semis();
                        if self.eat_sym("}") {
                            break;
                        }
                        let ksp = self.span();
                        let key = self.ident()?;
                        if key != "create" && key != "notify" && key != "update" {
                            once(&mut tseen, &key, ksp, "transition")?;
                        }
                        match key.as_str() {
                            "from" => from = Some(self.expr()?),
                            "to" => loop {
                                let f = self.ident()?;
                                self.expect_sym("=")?;
                                to.push((f, self.additive()?));
                                if !self.eat_sym(",") {
                                    break;
                                }
                            },
                            "allow" => allow = Some(self.expr()?),
                            "repeat" => repeat = Some((self.ident()?, ksp)),
                            "create" => {
                                let resource = self.ident()?;
                                self.expect_sym("{")?;
                                let mut values = vec![];
                                loop {
                                    self.semis();
                                    if self.eat_sym("}") {
                                        break;
                                    }
                                    let f = self.ident()?;
                                    self.expect_sym("=")?;
                                    values.push((f, self.unary()?));
                                }
                                effects.push(Effect::Create { resource, values, span: ksp });
                            }
                            "update" => {
                                let resource = self.ident()?;
                                self.expect_kw("where")?;
                                let mut matches = vec![];
                                loop {
                                    let f = self.ident()?;
                                    self.expect_sym("=")?;
                                    matches.push((f, self.unary()?));
                                    if !self.is_kw("and") {
                                        break;
                                    }
                                    self.next();
                                }
                                self.expect_sym("{")?;
                                let mut values = vec![];
                                loop {
                                    self.semis();
                                    if self.eat_sym("}") {
                                        break;
                                    }
                                    let f = self.ident()?;
                                    self.expect_sym("=")?;
                                    values.push((f, self.unary()?));
                                }
                                effects.push(Effect::Update { resource, matches, values, span: ksp });
                            }
                            "notify" => {
                                let to = self.unary()?;
                                effects.push(Effect::Notify { to, topic: self.string()?, span: ksp });
                            }
                            o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("transition 안 알 수 없는 항목 `{o}`"), sp)),
                        }
                    }
                    match (from, allow) {
                        (Some(from), Some(allow)) if !to.is_empty() => {
                            r.transitions.push(Transition { name, from, to, allow, repeat, effects, span: sp })
                        }
                        _ => return Err(Diag::new("MISSING_ITEM", "transition에는 from/to/allow가 모두 필요", sp)),
                    }
                }
                "unique" => r.uniques.push((self.ident_list()?, sp)),
                "check" => {
                    let n = self.ident()?;
                    self.expect_kw("when")?;
                    r.checks.push((n, self.expr()?, sp));
                }
                "invariant" => {
                    let n = self.ident()?;
                    self.expect_kw("per")?;
                    r.invariants.push((n.clone(), self.ident()?, sp));
                    if self.is_kw("deferred") {
                        self.next();
                        r.deferred_invariants.push(n);
                    }
                }
                "extension" => r.extensions.push(self.extension(sp)?),
                "docs" => {
                    self.expect_sym("{")?;
                    let mut d = Docs { summary: None, visibility: None, span: sp };
                    let mut dseen = BTreeSet::new();
                    loop {
                        self.semis();
                        if self.eat_sym("}") {
                            break;
                        }
                        let ksp = self.span();
                        let key = self.ident()?;
                        once(&mut dseen, &key, ksp, "docs")?;
                        match key.as_str() {
                            "summary" => d.summary = Some(self.string()?),
                            "visibility" => d.visibility = Some(self.ident()?),
                            o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("docs 안 알 수 없는 키 `{o}`"), sp)),
                        }
                    }
                    r.docs = Some(d);
                }
                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("resource 안 알 수 없는 항목 `{o}`"), sp)),
            }
        }
    }

    fn expose_read(&mut self, span: Span) -> R<ExposeRead> {
        let mut e = ExposeRead { span, ..Default::default() };
        let mut eseen = BTreeSet::new();
        self.expect_sym("{")?;
        loop {
            self.semis();
            if self.eat_sym("}") {
                return Ok(e);
            }
            let sp = self.span();
            let key = self.ident()?;
            if key != "traverse" {
                once(&mut eseen, &key, sp, "expose read")?;
            }
            match key.as_str() {
                "select" => e.select.extend(self.ident_list()?),
                "filter" => loop {
                    let fsp = self.span();
                    let f = self.ident()?;
                    self.expect_sym(".")?;
                    e.filter.push((f, self.ident()?, fsp));
                    if !self.eat_sym(",") {
                        break;
                    }
                },
                "sort" => e.sort.extend(self.ident_list()?),
                "traverse" => {
                    let rel = self.ident()?;
                    self.expect_sym("{")?;
                    self.semis();
                    self.expect_kw("select")?;
                    let sel = self.ident_list()?;
                    self.semis();
                    self.expect_sym("}")?;
                    e.traverse.push((rel, sel, sp));
                }
                "budget" => {
                    let mut b = Budget { span: sp, ..Default::default() };
                    let mut bseen = BTreeSet::new();
                    self.expect_sym("{")?;
                    loop {
                        self.semis();
                        if self.eat_sym("}") {
                            break;
                        }
                        let ksp = self.span();
                        let key = self.ident()?;
                        once(&mut bseen, &key, ksp, "budget")?;
                        match key.as_str() {
                            "rows" => b.rows = Some(self.int()?),
                            "depth" => b.depth = Some(self.int()?),
                            "deadline" => b.deadline_ms = Some(self.dur()?),
                            "cost" => b.cost = Some(self.int()?),
                            o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("budget 안 알 수 없는 키 `{o}`"), sp)),
                        }
                    }
                    e.budget = Some(b);
                }
                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("expose read 안 알 수 없는 항목 `{o}`"), sp)),
            }
        }
    }

    fn aggregate(&mut self, span: Span) -> R<Aggregate> {
        let name = self.ident()?;
        self.expect_sym(":")?;
        let ty = self.type_ref()?;
        let mut a = Aggregate {
            name,
            ty,
            input: vec![],
            source: None,
            source_access: None,
            group_key: None,
            where_: None,
            caller_filter: None,
            row_output: None,
            release: None,
            span,
        };
        let mut aseen = BTreeSet::new();
        self.expect_sym("{")?;
        loop {
            self.semis();
            if self.eat_sym("}") {
                return Ok(a);
            }
            let sp = self.span();
            let key = self.ident()?;
            once(&mut aseen, &key, sp, "aggregate")?;
            match key.as_str() {
                "input" => a.input = self.params_block()?,
                "source" => a.source = Some(self.ident()?),
                "sourceAccess" => {
                    let n = self.ident()?;
                    a.source_access = Some(if self.is_sym("(") { AccessRef::Call(n, self.call_args()?) } else { AccessRef::Name(n) });
                }
                "groupKey" => a.group_key = Some(self.ident()?),
                "where" => a.where_ = Some(self.expr()?),
                "callerFilter" => a.caller_filter = Some(self.ident()?),
                "rowOutput" => a.row_output = Some(self.ident()?),
                "release" => {
                    // `count` 또는 `sum(field)`/`min(field)`/`max(field)`. 호스트 형식과 같은 문자열로 남긴다.
                    let f = self.ident()?;
                    a.release = Some(if self.eat_sym("(") {
                        let field = self.ident()?;
                        self.expect_sym(")")?;
                        format!("{f}({field})")
                    } else {
                        f
                    });
                }
                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("aggregate 안 알 수 없는 키 `{o}`"), sp)),
            }
        }
    }

    fn extension(&mut self, span: Span) -> R<Extension> {
        let kind = self.ident()?;
        let name = self.ident()?;
        let mut x = Extension {
            kind,
            name,
            input: vec![],
            output: vec![],
            access: vec![],
            effect: String::new(),
            deadline_ms: None,
            implementation: String::new(),
            span,
        };
        let mut xseen = BTreeSet::new();
        self.expect_sym("{")?;
        loop {
            self.semis();
            if self.eat_sym("}") {
                return Ok(x);
            }
            let sp = self.span();
            let key = self.ident()?;
            once(&mut xseen, &key, sp, "extension")?;
            match key.as_str() {
                "input" => x.input = self.params_block()?,
                "output" => x.output = self.params_block()?,
                "access" => loop {
                    let r = self.ident()?;
                    self.expect_sym(".")?;
                    x.access.push((r, self.ident()?));
                    if !self.eat_sym(",") {
                        break;
                    }
                },
                "effect" => x.effect = self.ident()?,
                "deadline" => x.deadline_ms = Some(self.dur()?),
                "implementation" => x.implementation = self.string()?,
                o => return Err(Diag::new("PARSE_UNKNOWN_KEY", format!("extension 안 알 수 없는 키 `{o}`"), sp)),
            }
        }
    }

    fn call_args(&mut self) -> R<Vec<Expr>> {
        self.expect_sym("(")?;
        let mut v = vec![];
        if !self.is_sym(")") {
            loop {
                v.push(self.expr()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
        }
        self.expect_sym(")")?;
        Ok(v)
    }

    pub fn expr(&mut self) -> R<Expr> {
        let root = self.expression_depth == 0;
        if root {
            self.expression_nodes = 0;
        }
        let expr = self.nested_expression(Self::expression)?;
        if root && ast_depth(&expr) > MAX_AST_DEPTH {
            return Err(Diag::new("PARSE_NESTING", "정의식 AST 깊이 hard ceiling을 벗어남", self.span()));
        }
        Ok(expr)
    }

    fn nested_expression(&mut self, parse: impl FnOnce(&mut Self) -> R<Expr>) -> R<Expr> {
        if self.expression_depth >= MAX_EXPRESSION_DEPTH {
            return Err(Diag::new("PARSE_NESTING", "정의식 중첩 hard ceiling을 벗어남", self.span()));
        }
        self.expression_depth += 1;
        let result = parse(self);
        self.expression_depth -= 1;
        result
    }

    fn note_expression_node(&mut self) -> R<()> {
        if self.expression_nodes >= MAX_EXPR_NODES {
            return Err(Diag::new("PARSE_NESTING", format!("정의식이 {MAX_EXPR_NODES} AST node hard ceiling을 넘음"), self.span()));
        }
        self.expression_nodes += 1;
        Ok(())
    }

    fn expression(&mut self) -> R<Expr> {
        let mut v = vec![self.and()?];
        while self.is_kw("or") {
            self.next();
            v.push(self.and()?);
        }
        if v.len() == 1 {
            Ok(v.pop().unwrap())
        } else {
            self.note_expression_node()?;
            Ok(Expr::Or(v))
        }
    }
    fn and(&mut self) -> R<Expr> {
        let mut v = vec![self.not()?];
        while self.is_kw("and") {
            self.next();
            v.push(self.not()?);
        }
        if v.len() == 1 {
            Ok(v.pop().unwrap())
        } else {
            self.note_expression_node()?;
            Ok(Expr::And(v))
        }
    }
    fn not(&mut self) -> R<Expr> {
        if self.is_kw("not") {
            self.next();
            return self.nested_expression(|parser| {
                let expr = parser.not()?;
                parser.note_expression_node()?;
                Ok(Expr::Not(Box::new(expr)))
            });
        }
        self.cmp()
    }
    fn cmp(&mut self) -> R<Expr> {
        let l = self.additive()?;
        for op in ["=", "!=", ">=", "<=", ">", "<"] {
            if self.eat_sym(op) {
                let r = self.additive()?;
                self.note_expression_node()?;
                return Ok(Expr::Cmp(op, Box::new(l), Box::new(r)));
            }
        }
        if self.is_kw("in") {
            self.next();
            self.expect_sym("(")?;
            let mut v = vec![];
            loop {
                v.push(self.unary()?);
                if !self.eat_sym(",") {
                    break;
                }
            }
            self.expect_sym(")")?;
            self.note_expression_node()?;
            return Ok(Expr::In(Box::new(l), v));
        }
        Ok(l)
    }
    fn additive(&mut self) -> R<Expr> {
        let mut l = self.unary()?;
        loop {
            let sp = self.span();
            let op = if self.eat_sym("+") {
                "+"
            } else if self.eat_sym("-") {
                "-"
            } else {
                return Ok(l);
            };
            let r = self.unary()?;
            self.note_expression_node()?;
            l = Expr::Arith(op, Box::new(l), Box::new(r), sp);
        }
    }
    fn unary(&mut self) -> R<Expr> {
        let sp = self.span();
        match self.peek().clone() {
            Tok::Sym("(") => {
                self.next();
                let e = self.expr()?;
                self.expect_sym(")")?;
                Ok(e)
            }
            Tok::Int(n) => {
                self.next();
                self.note_expression_node()?;
                Ok(Expr::Int(n))
            }
            Tok::Str(s) => {
                self.next();
                self.note_expression_node()?;
                Ok(Expr::Str(s))
            }
            Tok::Ident(s) if s == "null" => {
                self.next();
                self.note_expression_node()?;
                Ok(Expr::Null)
            }
            Tok::Ident(s) if s == "true" || s == "false" => {
                self.next();
                self.note_expression_node()?;
                Ok(Expr::Bool(s == "true"))
            }
            Tok::Ident(s) if s == "now" => {
                self.next();
                self.note_expression_node()?;
                Ok(Expr::Now)
            }
            Tok::Ident(s) if s == "exists" => {
                self.next();
                let r = self.ident()?;
                self.expect_kw("where")?;
                let condition = self.expr()?;
                self.note_expression_node()?;
                Ok(Expr::Exists(r, Box::new(condition), sp))
            }
            Tok::Ident(s) => {
                self.next();
                if self.is_sym("(") {
                    let args = self.call_args()?;
                    self.note_expression_node()?;
                    return Ok(Expr::Call(s, args, sp));
                }
                let mut path = vec![s];
                while self.eat_sym(".") {
                    path.push(self.ident()?);
                }
                self.note_expression_node()?;
                Ok(Expr::Path(path, sp))
            }
            _ => self.err("식"),
        }
    }
}

fn ast_depth(root: &Expr) -> usize {
    let mut max_depth = 0;
    let mut pending = vec![(root, 0usize)];
    while let Some((expr, depth)) = pending.pop() {
        match expr {
            Expr::Or(children) | Expr::And(children) | Expr::Call(_, children, _) => {
                let child_depth = depth + 1;
                max_depth = max_depth.max(child_depth);
                pending.extend(children.iter().map(|child| (child, child_depth)));
            }
            Expr::Not(child) | Expr::Exists(_, child, _) => {
                let child_depth = depth + 1;
                max_depth = max_depth.max(child_depth);
                pending.push((child, child_depth));
            }
            Expr::Cmp(_, left, right) | Expr::Arith(_, left, right, _) => {
                let child_depth = depth + 1;
                max_depth = max_depth.max(child_depth);
                pending.push((left, child_depth));
                pending.push((right, child_depth));
            }
            Expr::In(left, items) => {
                let child_depth = depth + 1;
                max_depth = max_depth.max(child_depth);
                pending.push((left, child_depth));
                pending.extend(items.iter().map(|item| (item, child_depth)));
            }
            Expr::Path(_, _) | Expr::Null | Expr::Now | Expr::Int(_) | Expr::Bool(_) | Expr::Str(_) => {}
        }
    }
    max_depth
}
