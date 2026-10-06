//! AIP surface syntax: lexer, AST and recursive-descent parser.
//!
//! The grammar is specified in `spec/grammar.md`; this crate is its executable
//! form. Parsing never resolves names or types; that is `aip-sema`'s job.

pub mod ast;
pub mod diag;
pub mod lexer;
pub mod parser;

pub use diag::{Diagnostic, Severity, Span};
pub use parser::{parse_file, parse_snippet};
