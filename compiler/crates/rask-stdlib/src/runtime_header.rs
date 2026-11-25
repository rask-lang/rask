// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! The C runtime's prototypes, as `rask_runtime.h` writes them.
//!
//! A runtime function's signature is written once, in the header the C
//! compiler checks the definition against. Lowering asks it whether a name is
//! a runtime symbol it can call directly; native codegen reads the registers
//! from it. Neither keeps a second copy.
//!
//! A prototype marked `RASK_PANICS` can panic, so codegen records the call's
//! source location before making it.

use std::collections::{HashMap, HashSet};
use std::sync::OnceLock;

/// One prototype as written: the C spelling of its return type and of each
/// parameter (type and name).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Written {
    pub ret: String,
    pub params: Vec<String>,
    /// Marked `RASK_PANICS`.
    pub panics: bool,
}

/// What the header declares: its functions by name, and the names it gives
/// function-pointer types (`typedef void (*RaskEnsureFn)(void *ctx)`).
#[derive(Default)]
struct Header {
    functions: HashMap<String, Written>,
    fn_pointers: HashSet<String>,
}

fn header() -> &'static Header {
    static CACHE: OnceLock<Header> = OnceLock::new();
    CACHE.get_or_init(|| parse(include_str!("../../../runtime/rask_runtime.h")))
}

fn all() -> &'static HashMap<String, Written> {
    &header().functions
}

fn parse(header: &str) -> Header {
    // Comments and preprocessor lines out, then one declaration per `;`.
    let mut text = String::new();
    let mut in_block = false;
    for line in header.lines() {
        let mut line = line;
        if in_block {
            match line.find("*/") {
                Some(end) => {
                    line = &line[end + 2..];
                    in_block = false;
                }
                None => continue,
            }
        }
        if let Some(start) = line.find("/*") {
            if !line[start..].contains("*/") {
                in_block = true;
            }
            line = &line[..start];
        }
        let line = line.split("//").next().unwrap_or("");
        if line.trim_start().starts_with('#') {
            continue;
        }
        text.push_str(line);
        text.push(' ');
    }
    let mut out = Header::default();
    for decl in text.split(|c| c == ';' || c == '{' || c == '}') {
        let decl = decl.trim();
        if decl.contains("typedef") {
            if let Some(at) = decl.find("(*") {
                let rest = &decl[at + 2..];
                if let Some(end) = rest.find(')') {
                    out.fn_pointers.insert(rest[..end].trim().to_string());
                }
            }
            continue;
        }
        let (Some(open), Some(close)) = (decl.find('('), decl.rfind(')')) else { continue };
        let head = decl[..open].trim();
        let Some(name_at) = head.rfind(|c: char| !(c.is_alphanumeric() || c == '_')) else {
            continue;
        };
        let name = &head[name_at + 1..];
        if name.is_empty() {
            continue;
        }
        let ret_words: Vec<&str> = head[..name_at + 1].split_whitespace().collect();
        let panics = ret_words.contains(&"RASK_PANICS");
        let ret = ret_words.iter().filter(|w| **w != "RASK_PANICS").copied().collect::<Vec<_>>().join(" ");
        let params: Vec<String> = decl[open + 1..close]
            .split(',')
            .map(|p| p.trim().to_string())
            .filter(|p| !p.is_empty() && p != "void")
            .collect();
        out.functions.insert(name.to_string(), Written { ret, params, panics });
    }
    out
}

/// The prototype `symbol` has in the header.
pub fn prototype(symbol: &str) -> Option<&'static Written> {
    all().get(symbol)
}

/// Does the header declare `symbol`?
pub fn declares(symbol: &str) -> bool {
    all().contains_key(symbol)
}

/// Is `name` a function-pointer type the header names with a typedef?
pub fn is_fn_pointer_type(name: &str) -> bool {
    header().fn_pointers.contains(name)
}

/// Every symbol the header declares.
pub fn symbols() -> impl Iterator<Item = &'static str> {
    all().keys().map(String::as_str)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_panic_mark() {
        let h = parse("RASK_PANICS int64_t rask_a(int64_t x);\nvoid rask_b(void);").functions;
        assert!(h["rask_a"].panics);
        assert_eq!(h["rask_a"].ret, "int64_t");
        assert_eq!(h["rask_a"].params, vec!["int64_t x".to_string()]);
        assert!(!h["rask_b"].panics);
        assert!(h["rask_b"].params.is_empty());
    }

    #[test]
    fn reads_the_real_header() {
        assert!(declares("rask_vec_push"));
        assert!(all().len() > 400);
        assert!(is_fn_pointer_type("RaskEnsureFn"));
    }
}
