//! Parser for the Rask language.
//!
//! Transforms a token stream into an abstract syntax tree.

mod hints;
mod parser;

pub use parser::{ParseError, ParseResult, Parser};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_all_examples() {
        let examples_dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent().unwrap()
            .parent().unwrap()
            .parent().unwrap()
            .join("examples");

        for entry in std::fs::read_dir(&examples_dir).expect("examples directory not found") {
            let path = entry.unwrap().path();
            if path.extension().map(|e| e == "rask").unwrap_or(false) {
                let src = std::fs::read_to_string(&path)
                    .expect(&format!("Failed to read {}", path.display()));
                let lex_result = rask_lexer::Lexer::new(&src).tokenize();
                assert!(lex_result.is_ok(), "Lex errors in {}: {:?}", path.display(), lex_result.errors);
                let parse_result = Parser::new(lex_result.tokens).parse();
                assert!(parse_result.is_ok(), "Parse errors in {}: {:?}", path.display(), parse_result.errors);
            }
        }
    }
}
