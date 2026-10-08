// SPDX-License-Identifier: (MIT OR Apache-2.0)
//! The stdlib follows the ownership rules it is compiled under.
//!
//! The ownership pass checks the program on every compile and reads only the
//! stdlib's signatures, so nothing ever checked the stdlib's own bodies. The
//! first run of the checker over them found eleven violations: borrowed
//! parameters given away, a loop's lent elements kept, a `Vec` element copied
//! out by value. This is the check, run where the stdlib is written rather
//! than on every compile.

use rask_compiler::{check_source, CfgConfig, CompilerConfig};

#[test]
fn stdlib_bodies_pass_the_ownership_checker() {
    let config = CompilerConfig { cfg: CfgConfig::from_host("debug", vec![]) };
    let out = check_source("probe.rk", "func main() {}\n", &config);
    let checked = out.result.expect("an empty program checks");
    let stdlib = rask_stdlib::StubRegistry::typecheck_decls();
    let result = rask_ownership::check_ownership_with_stdlib(&checked.typed, &stdlib, &stdlib);

    let sources: Vec<(&str, &str, u16)> = rask_stdlib::stubs::stub_sources().collect();
    let place = |span: rask_ast::Span| {
        sources
            .iter()
            .find(|(_, _, id)| *id == span.file_id)
            .map(|(name, src, _)| {
                let line = src[..span.start.min(src.len())].matches('\n').count() + 1;
                format!("stdlib/{}:{}", name, line)
            })
            .unwrap_or_else(|| format!("file {} @{}", span.file_id, span.start))
    };
    let found: Vec<String> = result
        .errors
        .iter()
        .map(|e| format!("{}: {:?}", place(e.span), e.kind))
        .collect();
    assert!(
        found.is_empty(),
        "the stdlib breaks the ownership rules in {} place(s):\n  {}",
        found.len(),
        found.join("\n  ")
    );
}
