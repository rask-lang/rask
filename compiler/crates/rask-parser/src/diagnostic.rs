// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How the parser's errors read to the user.

use rask_diagnostics::{Diagnostic, ToDiagnostic};

impl ToDiagnostic for crate::ParseError {
    fn to_diagnostic(&self) -> Diagnostic {
        let mut diag = Diagnostic::error(&self.message)
            .with_code("E0100")
            .with_primary(self.span, "here");

        if let Some(ref hint) = self.hint {
            diag = diag
                .with_help(hint.as_str())
                .with_fix(hint.as_str())
                .with_why(self.why.as_deref()
                    .unwrap_or("the parser expected valid syntax at this position"));
        }

        diag
    }
}
