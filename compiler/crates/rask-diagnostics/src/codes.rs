//! Error code registry.
//!
//! Maps error codes (E0001, E0308, etc.) to titles and categories.
//! Used by `rask explain <code>` and for error display.

use std::collections::HashMap;

/// Registry of all known error codes.
pub struct ErrorCodeRegistry {
    codes: HashMap<&'static str, ErrorCodeInfo>,
}

/// Information about a single error code.
pub struct ErrorCodeInfo {
    pub code: &'static str,
    pub title: &'static str,
    pub category: ErrorCategory,
}

/// Error category for grouping.
#[derive(Debug, Clone, Copy)]
pub enum ErrorCategory {
    Syntax,
    Resolution,
    Type,
    Trait,
    Ownership,
}

impl std::fmt::Display for ErrorCategory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ErrorCategory::Syntax => write!(f, "Syntax"),
            ErrorCategory::Resolution => write!(f, "Resolution"),
            ErrorCategory::Type => write!(f, "Type"),
            ErrorCategory::Trait => write!(f, "Trait"),
            ErrorCategory::Ownership => write!(f, "Ownership"),
        }
    }
}

macro_rules! register_codes {
    ($($code:literal => ($title:literal, $cat:expr)),* $(,)?) => {{
        let mut map = HashMap::new();
        $(
            map.insert($code, ErrorCodeInfo {
                code: $code,
                title: $title,
                category: $cat,
            });
        )*
        map
    }};
}

impl Default for ErrorCodeRegistry {
    fn default() -> Self {
        use ErrorCategory::*;

        Self {
            codes: register_codes! {
                // Lexer errors (E00xx)
                "E0001" => ("unexpected character", Syntax),
                "E0002" => ("unterminated string literal", Syntax),
                "E0003" => ("invalid escape sequence", Syntax),
                "E0004" => ("invalid number format", Syntax),

                // Parser errors (E01xx)
                "E0100" => ("unexpected token", Syntax),
                "E0101" => ("expected token not found", Syntax),
                "E0102" => ("invalid syntax", Syntax),

                // Resolver errors (E02xx)
                "E0200" => ("undefined symbol", Resolution),
                "E0201" => ("duplicate definition", Resolution),
                "E0202" => ("circular dependency", Resolution),
                "E0203" => ("symbol not visible", Resolution),
                "E0204" => ("break outside of loop", Resolution),
                "E0205" => ("continue outside of loop", Resolution),
                "E0206" => ("return outside of function", Resolution),
                "E0207" => ("unknown package", Resolution),
                "E0208" => ("shadows import", Resolution),
                "E0209" => ("shadows built-in", Resolution),

                // Type errors (E03xx)
                "E0308" => ("mismatched types", Type),
                "E0309" => ("undefined type", Type),
                "E0310" => ("arity mismatch", Type),
                "E0311" => ("type is not callable", Type),
                "E0312" => ("no such field", Type),
                "E0313" => ("no such method", Type),
                "E0314" => ("infinite type", Type),
                "E0315" => ("cannot infer type", Type),
                "E0316" => ("invalid try context", Type),
                "E0317" => ("try outside function", Type),
                "E0318" => ("missing return statement", Type),

                // Trait errors (E07xx)
                "E0700" => ("trait bound not satisfied", Trait),
                "E0701" => ("missing trait method", Trait),
                "E0702" => ("method signature mismatch", Trait),
                "E0703" => ("unknown trait", Trait),
                "E0704" => ("conflicting trait methods", Trait),

                // Ownership errors (E08xx)
                "E0800" => ("use after move", Ownership),
                "E0801" => ("borrow conflict", Ownership),
                "E0802" => ("mutate while borrowed", Ownership),
                "E0803" => ("instant borrow escapes", Ownership),
                "E0804" => ("borrow escapes scope", Ownership),
                "E0805" => ("resource not consumed", Ownership),
            },
        }
    }
}

impl ErrorCodeRegistry {
    pub fn get(&self, code: &str) -> Option<&ErrorCodeInfo> {
        self.codes.get(code)
    }

    pub fn all(&self) -> impl Iterator<Item = &ErrorCodeInfo> {
        self.codes.values()
    }
}
