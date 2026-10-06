//! The one way source becomes Core IR: every consumer (`aip check`, `run`, `ddl`, the conformance runner,
//! the end-to-end tests) calls [`check_source`], so they all see the same diagnostics.
//!
//! ```text
//! parse -> expand -> sema (names, types: frontend errors)
//!   frontend errors?  stop
//!   -> to_core -> ir::validate (well-formed) -> ir::analyze (semantic rules)
//!   -> one list, positions from the SourceMap, sorted by line and column
//! ```
//!
//! Only frontend errors (unknown names, type mismatches, malformed syntax) stop lowering. A program that breaks a
//! semantic rule still lowers, so the rule is reported from the IR, where a TS or Python frontend gets it too.

use crate::{analyze, has_errors, to_core};
use aip_ir as ir;
use aip_syntax::ast::File;
use aip_syntax::{Diagnostic, Severity, Span};

pub struct Checked {
    pub diagnostics: Vec<Diagnostic>,
    /// False when the source did not parse; `diagnostics` then holds the one parse error.
    pub parsed: bool,
    /// The lowered program, present when sema found no error; semantic and structural errors are in `diagnostics`.
    pub lowered: Option<(ir::Program, ir::SourceMap)>,
}

impl Checked {
    pub fn has_errors(&self) -> bool {
        has_errors(&self.diagnostics)
    }

    /// The program when nothing at all is an error: what a backend may compile.
    pub fn core(&self) -> Option<&(ir::Program, ir::SourceMap)> {
        if self.has_errors() { None } else { self.lowered.as_ref() }
    }

    pub fn into_core(self) -> Option<(ir::Program, ir::SourceMap)> {
        if self.has_errors() { None } else { self.lowered }
    }
}

pub fn check_source(src: &str) -> Checked {
    match aip_syntax::parse_file(src) {
        Ok(ast) => check_ast(&ast),
        Err(d) => Checked { diagnostics: vec![d], parsed: false, lowered: None },
    }
}

pub fn check_ast(ast: &File) -> Checked {
    let expanded = crate::lower::expand(ast);
    let analysis = analyze(&expanded);
    let mut diagnostics = analysis.diagnostics.clone();
    if has_errors(&diagnostics) {
        return Checked { diagnostics, parsed: true, lowered: None };
    }
    let (core, map) = to_core::to_core(&analysis);
    let mut ir_diags = ir::validate::validate(&core);
    ir_diags.extend(ir::analyze::analyze(&core));
    diagnostics.extend(ir_diags.iter().map(|d| to_diagnostic(d, &map)));
    diagnostics.sort_by_key(|d| (d.span.line, d.span.col));
    diagnostics.dedup_by(|a, b| a.code == b.code && a.span == b.span && a.message == b.message);
    Checked { diagnostics, parsed: true, lowered: Some((core, map)) }
}

/// An IR diagnostic as the frontend prints it: the line and column of the closest path the source map knows.
pub fn to_diagnostic(d: &ir::validate::IrDiagnostic, map: &ir::SourceMap) -> Diagnostic {
    let (line, col) = ir::locate(map, &d.path);
    let mut message = d.message.clone();
    if let Some(rel) = &d.related {
        let (rline, _) = ir::locate(map, rel);
        message = message.replace(rel.as_str(), &format!("line {rline}"));
    }
    Diagnostic {
        severity: if d.is_warning() { Severity::Warning } else { Severity::Error },
        code: d.code.clone(),
        message,
        // the IR carries no byte offsets
        span: Span { start: 0, end: 0, line, col },
        help: d.help.clone(),
    }
}
