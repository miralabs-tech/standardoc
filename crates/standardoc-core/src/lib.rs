//! Standardoc core: data model, index, DSL evaluator, validator, scanner and watcher.
//!
//! This crate is deliberately split from language-specific AST providers:
//! the `LanguageProvider` trait lives in `standardoc-lang` and concrete
//! implementations live in sibling crates (`standardoc-lang-ts`, etc.).

pub mod config;
pub mod dsl;
pub mod emit;
pub mod extractor;
pub mod lang;
pub mod lang_def;
pub mod lang_regex;
pub mod materialize;
pub mod model;
pub mod pages;
pub mod pipeline;
pub mod scanner;
pub mod validator;
pub mod virtual_annotation;
pub mod watcher;

pub use crate::config::*;
pub use crate::lang::*;
pub use crate::model::*;
