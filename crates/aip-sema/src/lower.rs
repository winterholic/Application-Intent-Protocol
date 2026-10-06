//! L3 → L2 lowering on the AST: domain forms that are pure sugar expand into
//! ordinary declarations before analysis, so the checker and the backend
//! never need to know about them. Generated nodes get fresh spans (unique
//! start/end) but keep the line/column of the form they came from, so
//! diagnostics still point at the user's source.

use aip_syntax::Span;
use aip_syntax::ast::*;

struct Fresh {
    next: u32,
    at: Span,
}

impl Fresh {
    fn span(&mut self) -> Span {
        self.next += 2;
        Span { start: self.next, end: self.next + 1, line: self.at.line, col: self.at.col }
    }

    fn ident(&mut self, name: &str) -> Ident {
        Ident { name: name.to_string(), span: self.span() }
    }

    fn name(&mut self, name: &str) -> Expr {
        let id = self.ident(name);
        Expr { span: id.span, kind: ExprKind::Name(id) }
    }

    fn field(&mut self, base: Expr, f: &str) -> Expr {
        let id = self.ident(f);
        Expr { span: self.span(), kind: ExprKind::Field(Box::new(base), id) }
    }

    fn kw(&mut self, k: Kw) -> Expr {
        Expr { span: self.span(), kind: ExprKind::Kw(k) }
    }

    /// Deep copy with fresh spans; bare names in `fields` become `root.name`.
    fn copy(&mut self, e: &Expr, fields: &[String], root: Option<&str>) -> Expr {
        let span = self.span();
        let kind = match &e.kind {
            ExprKind::Name(n) if root.is_some() && fields.contains(&n.name) => {
                let base = self.name(root.unwrap_or_default());
                return self.field(base, &n.name);
            }
            ExprKind::Name(n) => ExprKind::Name(Ident { name: n.name.clone(), span }),
            ExprKind::Field(b, f) => ExprKind::Field(Box::new(self.copy(b, fields, root)), self.ident(&f.name)),
            ExprKind::Call(c) => ExprKind::Call(CallExpr {
                callee: c.callee.clone(),
                args: c.args.iter().map(|a| Arg { name: a.name.clone(), value: self.copy(&a.value, fields, root) }).collect(),
                span: self.span(),
            }),
            ExprKind::Neg(x) => ExprKind::Neg(Box::new(self.copy(x, fields, root))),
            ExprKind::Not(x) => ExprKind::Not(Box::new(self.copy(x, fields, root))),
            ExprKind::Binary(op, l, r) => ExprKind::Binary(*op, Box::new(self.copy(l, fields, root)), Box::new(self.copy(r, fields, root))),
            ExprKind::InList(x, items) => {
                ExprKind::InList(Box::new(self.copy(x, fields, root)), items.iter().map(|i| self.copy(i, fields, root)).collect())
            }
            ExprKind::InExpr(x, r) => ExprKind::InExpr(Box::new(self.copy(x, fields, root)), Box::new(self.copy(r, fields, root))),
            ExprKind::Is(x, n) => ExprKind::Is(Box::new(self.copy(x, fields, root)), n.clone()),
            ExprKind::Exists(se) => ExprKind::Exists(Box::new(self.copy_set(se, fields, root))),
            ExprKind::If(a, b, c) => {
                ExprKind::If(Box::new(self.copy(a, fields, root)), Box::new(self.copy(b, fields, root)), Box::new(self.copy(c, fields, root)))
            }
            other => other.clone(),
        };
        Expr { kind, span }
    }

    fn copy_set(&mut self, se: &SetExpr, fields: &[String], root: Option<&str>) -> SetExpr {
        SetExpr {
            source: self.copy(&se.source, fields, root),
            alias: se.alias.clone(),
            filter: se.filter.as_ref().map(|f| Box::new(self.copy(f, fields, None))),
            span: self.span(),
        }
    }
}

fn entity<'f>(file: &'f File, name: &str) -> Option<&'f EntityDecl> {
    file.decls.iter().find_map(|d| match d {
        Decl::Entity(e) if e.name.name == name => Some(e),
        _ => None,
    })
}

fn fields(e: &EntityDecl) -> Vec<&FieldDecl> {
    e.members
        .iter()
        .filter_map(|m| if let EntityMember::Field(f) = m { Some(f) } else { None })
        .filter(|f| !matches!(f.ty.kind, TypeKind::Many(_)))
        .collect()
}

fn is_plain(f: &FieldDecl) -> bool {
    !f.mods.iter().any(|m| matches!(m, FieldMod::Counter { .. } | FieldMod::Via(_)))
}

pub fn expand(file: &File) -> File {
    let mut out = file.clone();
    let mut fresh = Fresh { next: 0x4000_0000, at: Span::default() };
    for d in &file.decls {
        if let Decl::Expose(x) = d {
            fresh.at = x.span;
            out.decls.extend(expose(file, x, &mut fresh));
        }
    }
    out
}

fn allow(expr: Expr, span: Span) -> Allow {
    Allow { cond: expr, code: None, span }
}

fn param(fresh: &mut Fresh, name: &str, ty: &TypeExpr) -> Param {
    Param { name: fresh.ident(name), ty: ty.clone(), default: None, span: fresh.span() }
}

fn sel_all(fresh: &mut Fresh, fs: &[&FieldDecl]) -> Selection {
    let mut items = vec![SelItem { name: fresh.ident("id"), value: None, sub: None }];
    for f in fs {
        if matches!(f.ty.kind, TypeKind::Name(ref q) if q.parts.len() == 1) || matches!(f.ty.kind, TypeKind::Refined { .. }) {
            items.push(SelItem { name: fresh.ident(&f.name.name), value: None, sub: None });
        }
    }
    Selection { items, span: fresh.span() }
}

/// `expose E { read / create / update / delete }` → Get/List/Create/Update/Delete intents.
fn expose(file: &File, x: &ExposeDecl, fresh: &mut Fresh) -> Vec<Decl> {
    let Some(ent) = entity(file, &x.entity.name) else { return Vec::new() };
    let e = &ent.name.name;
    let fs = fields(ent);
    let field_names: Vec<String> = fs.iter().map(|f| f.name.name.clone()).collect();
    let scalar_fields: Vec<&FieldDecl> = fs.iter().copied().filter(|f| is_plain(f) && !f.ty.optional || is_plain(f)).collect();
    let mut out = Vec::new();
    let span = x.span;
    // the parent scope for listing: the first required reference
    let parent = fs.iter().find(|f| {
        !f.ty.optional && matches!(&f.ty.kind, TypeKind::Name(q) if q.parts.len() == 1 && file.decls.iter().any(|d| matches!(d, Decl::Entity(p) if p.name.name == q.parts[0].name)))
    });
    if let Some(read) = &x.read {
        let cond = match read {
            Some(c) => fresh.copy(c, &field_names, Some("target")),
            None => fresh.kw(Kw::Public),
        };
        let target_ty = TypeExpr { kind: TypeKind::Name(QualName { parts: vec![fresh.ident(e)], span }), optional: false, span };
        out.push(Decl::Query(QueryDecl {
            internal: false,
            drafts: false,
            cross_tenant: false,
            name: fresh.ident(&format!("Get{e}")),
            params: vec![param(fresh, "target", &target_ty)],
            cached: None,
            limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(allow(cond, span)),
            fetches: Vec::new(),
            from: Some(FromClause::Param(fresh.ident("target"))),
            filter: None,
            group_by: Vec::new(),
            sort: None,
            page: None,
            plan: None,
            consistency: None,
            select: sel_all(fresh, &scalar_fields),
            touches: Vec::new(),
            span,
        }));
        if let Some(p) = parent {
            let pname = p.name.name.clone();
            let cond = match read {
                Some(c) => {
                    // list-level condition cannot reference a single row; keep only parent-scoped rules
                    fresh.copy(c, &[], None)
                }
                None => fresh.kw(Kw::Public),
            };
            let order_field = fs.iter().find(|f| f.mods.iter().any(|m| matches!(m, FieldMod::Position { .. }))).map(|f| f.name.name.clone());
            let key = match &order_field {
                Some(f) => {
                    let b = fresh.name("x");
                    fresh.field(b, f)
                }
                None => {
                    let b = fresh.name("x");
                    fresh.field(b, "id")
                }
            };
            let filter = {
                let b = fresh.name("x");
                let l = fresh.field(b, &pname);
                let r = fresh.name(&pname);
                Expr { span: fresh.span(), kind: ExprKind::Binary(BinOp::Eq, Box::new(l), Box::new(r)) }
            };
            out.push(Decl::Query(QueryDecl {
                internal: false,
                drafts: false,
                cross_tenant: false,
                name: fresh.ident(&format!("List{e}")),
                params: vec![param(fresh, &pname, &TypeExpr { optional: false, ..p.ty.clone() })],
                cached: None,
                limits: Vec::new(),
                lets: Vec::new(),
                allow: Some(allow(cond, span)),
                fetches: Vec::new(),
                from: Some(FromClause::Entity { entity: fresh.ident(e), alias: fresh.ident("x") }),
                filter: Some(filter),
                group_by: Vec::new(),
                sort: Some(Sort::Keys(vec![SortKey { expr: key, desc: false }])),
                page: Some(Page { size: 50, offset_max_page: None, span }),
                plan: None,
                consistency: None,
                select: sel_all(fresh, &scalar_fields),
                touches: Vec::new(),
                span,
            }));
        }
    }
    if let Some((cond, cols)) = &x.create {
        let params: Vec<Param> =
            cols.iter().filter_map(|c| fs.iter().find(|f| f.name.name == c.name).map(|f| param(fresh, &f.name.name, &f.ty))).collect();
        let assigns = cols.iter().map(|c| FieldAssign::Named { name: fresh.ident(&c.name), value: None }).collect();
        let row = fresh.ident("row");
        let ret = fresh.name("row");
        out.push(Decl::Command(CommandDecl {
            internal: false,
            cross_tenant: false,
            name: fresh.ident(&format!("Create{e}")),
            params,
            idempotent: Some(None),
            audited: false,
            limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(allow(fresh.copy(cond, &[], None), span)),
            requires: Vec::new(),
            body: Some(Block { stmts: vec![Stmt::Insert { entity: fresh.ident(e), from: None, fields: assigns, bind: Some(row), span }], span }),
            emits: Vec::new(),
            returns: Some((ret, Some(Selection { items: vec![SelItem { name: fresh.ident("id"), value: None, sub: None }], span }))),
            span,
        }));
    }
    if let Some((cond, cols)) = &x.update {
        let target_ty = TypeExpr { kind: TypeKind::Name(QualName { parts: vec![fresh.ident(e)], span }), optional: false, span };
        let mut params = vec![param(fresh, "target", &target_ty)];
        params.extend(cols.iter().filter_map(|c| fs.iter().find(|f| f.name.name == c.name).map(|f| param(fresh, &f.name.name, &f.ty))));
        let assigns: Vec<Assign> = cols
            .iter()
            .map(|c| {
                let base = fresh.name("target");
                let target = fresh.field(base, &c.name);
                let value = fresh.name(&c.name);
                Assign { target, op: AssignOp::Set, value, span }
            })
            .collect();
        out.push(Decl::Command(CommandDecl {
            internal: false,
            cross_tenant: false,
            name: fresh.ident(&format!("Update{e}")),
            params,
            idempotent: None,
            audited: false,
            limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(allow(fresh.copy(cond, &field_names, Some("target")), span)),
            requires: Vec::new(),
            body: Some(Block { stmts: vec![Stmt::Set { assigns, span }], span }),
            emits: Vec::new(),
            returns: None,
            span,
        }));
    }
    if let Some(cond) = &x.delete {
        let target_ty = TypeExpr { kind: TypeKind::Name(QualName { parts: vec![fresh.ident(e)], span }), optional: false, span };
        let filter = {
            let l = fresh.name("x");
            let r = fresh.name("target");
            Expr { span: fresh.span(), kind: ExprKind::Binary(BinOp::Eq, Box::new(l), Box::new(r)) }
        };
        let src = fresh.name(e);
        out.push(Decl::Command(CommandDecl {
            internal: false,
            cross_tenant: false,
            name: fresh.ident(&format!("Delete{e}")),
            params: vec![param(fresh, "target", &target_ty)],
            idempotent: None,
            audited: false,
            limits: Vec::new(),
            lets: Vec::new(),
            allow: Some(allow(fresh.copy(cond, &field_names, Some("target")), span)),
            requires: Vec::new(),
            body: Some(Block {
                stmts: vec![Stmt::Delete {
                    target: SetExpr { source: src, alias: Some(fresh.ident("x")), filter: Some(Box::new(filter)), span },
                    span,
                }],
                span,
            }),
            emits: Vec::new(),
            returns: None,
            span,
        }));
    }
    out
}
