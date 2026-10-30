// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! How the lexer's errors read to the user.

use rask_diagnostics::{Diagnostic, ToDiagnostic};

impl ToDiagnostic for crate::LexError {
    fn to_diagnostic(&self) -> Diagnostic {
        let mut diag = Diagnostic::error(&self.message)
            .with_code(self.code)
            .with_primary(self.span, self.label);

        if let Some(ref hint) = self.hint {
            diag = diag
                .with_help(hint.as_str())
                .with_fix(hint.as_str())
                .with_why(self.why);
        }

        diag
    }
}
