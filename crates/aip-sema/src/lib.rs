//! Semantic analysis: name resolution, typing and the static rules of
//! `docs/design/04-safety-matrix.md` that can be decided per declaration.

pub mod check;
pub mod lower;
pub mod model;
pub mod pipeline;
pub mod to_core;
pub mod ty;

use aip_syntax::ast::File;
use aip_syntax::{Diagnostic, Severity};

pub struct Analysis<'a> {
    pub model: model::Model<'a>,
    pub diagnostics: Vec<Diagnostic>,
    /// Expression types keyed by `(span.start, span.end)`.
    pub types: std::collections::HashMap<(u32, u32), ty::Ty>,
}

impl Analysis<'_> {
    pub fn ty_of(&self, e: &aip_syntax::ast::Expr) -> ty::Ty {
        self.types.get(&(e.span.start, e.span.end)).cloned().unwrap_or(ty::Ty::Unknown)
    }
}

pub fn analyze(file: &File) -> Analysis<'_> {
    let mut model = model::Model::build(file);
    let (checker_diags, events, types) = {
        let mut checker = check::Checker::new(&model);
        checker.run();
        (checker.diags, checker.events, checker.types)
    };
    model.events = events;
    let mut diagnostics = std::mem::take(&mut model.diags);
    diagnostics.extend(checker_diags);
    diagnostics.sort_by_key(|d| (d.span.line, d.span.col));
    diagnostics.dedup_by(|a, b| a.code == b.code && a.span == b.span && a.message == b.message);
    Analysis { model, diagnostics, types }
}

pub fn has_errors(diags: &[Diagnostic]) -> bool {
    diags.iter().any(|d| d.severity == Severity::Error)
}
