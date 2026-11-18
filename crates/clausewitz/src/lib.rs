//! Lossless parser for Paradox (Clausewitz) script files.
//!
//! ```
//! let parsed = clausewitz::parse("country = { tag = GER color = rgb { 1 2 3 } }");
//! assert!(parsed.is_clean());
//! let country = parsed.root.get("country").unwrap().value.as_block().unwrap();
//! assert_eq!(country.get("tag").unwrap().value.as_str(), Some("GER"));
//! ```
//!
//! The parser never panics on malformed input: it reports [`Diagnostic`]s and returns the best tree it can build.

pub mod ast;
pub mod diag;
mod lexer;
mod parser;
pub mod text;

pub use ast::{Block, Entry, Item, Operator, Scalar, Span, Value};
pub use diag::{DiagKind, Diagnostic, LineIndex};
pub use parser::{Parsed, parse};
pub use text::decode;
