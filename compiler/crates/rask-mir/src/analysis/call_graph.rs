// SPDX-License-Identifier: (MIT OR Apache-2.0)

//! Call graph construction — maps caller→callee relationships across MIR functions.
//!
//! Used by the inlining pass to determine call counts and detect recursion.

use std::collections::{HashMap, HashSet};

use crate::{MirFunction, MirStmtKind};

/// Call graph over the MIR program.
///
/// Maps function names to their callees and tracks call counts.
#[derive(Debug)]
pub struct CallGraph {
    /// function name → set of callee names called from it
    pub callees: HashMap<String, HashSet<String>>,
    /// callee name → number of call sites across all functions
    pub call_count: HashMap<String, u32>,
    /// function name → MIR statement count (for size heuristic)
    pub stmt_count: HashMap<String, usize>,
    /// Set of functions that are (mutually) recursive
    pub recursive: HashSet<String>,
}

impl CallGraph {
    /// Build a call graph from the full set of MIR functions.
    pub fn build(fns: &[MirFunction]) -> Self {
        let fn_names: HashSet<String> = fns.iter().map(|f| f.name.clone()).collect();

        let mut callees: HashMap<String, HashSet<String>> = HashMap::new();
        let mut call_count: HashMap<String, u32> = HashMap::new();
        let mut stmt_count: HashMap<String, usize> = HashMap::new();

        for func in fns {
            let mut func_callees = HashSet::new();
            let mut total_stmts = 0usize;

            for block in &func.blocks {
                total_stmts += block.statements.len();
                for stmt in &block.statements {
                    if let MirStmtKind::Call { func: callee, .. } = &stmt.kind {
                        // Only track calls to functions we own (not extern/runtime)
                        if !callee.is_extern && fn_names.contains(&callee.name) {
                            func_callees.insert(callee.name.clone());
                            *call_count.entry(callee.name.clone()).or_insert(0) += 1;
                        }
                    }
                }
            }

            stmt_count.insert(func.name.clone(), total_stmts);
            callees.insert(func.name.clone(), func_callees);
        }

        let recursive = find_recursive(&callees);

        CallGraph {
            callees,
            call_count,
            stmt_count,
            recursive,
        }
    }

    /// True if the function is called exactly once across the entire program.
    pub fn called_once(&self, name: &str) -> bool {
        self.call_count.get(name) == Some(&1)
    }

    /// True if the function is (mutually) recursive.
    pub fn is_recursive(&self, name: &str) -> bool {
        self.recursive.contains(name)
    }

    /// MIR statement count for a function.
    pub fn statement_count(&self, name: &str) -> usize {
        self.stmt_count.get(name).copied().unwrap_or(0)
    }
}

/// Find all functions participating in cycles (direct or mutual recursion).
fn find_recursive(callees: &HashMap<String, HashSet<String>>) -> HashSet<String> {
    let mut recursive = HashSet::new();
    let mut visited = HashSet::new();
    let mut on_stack = HashSet::new();

    for name in callees.keys() {
        if !visited.contains(name.as_str()) {
            dfs_find_cycles(name, callees, &mut visited, &mut on_stack, &mut recursive);
        }
    }

    recursive
}

fn dfs_find_cycles(
    node: &str,
    callees: &HashMap<String, HashSet<String>>,
    visited: &mut HashSet<String>,
    on_stack: &mut HashSet<String>,
    recursive: &mut HashSet<String>,
) {
    visited.insert(node.to_string());
    on_stack.insert(node.to_string());

    if let Some(edges) = callees.get(node) {
        for callee in edges {
            if on_stack.contains(callee.as_str()) {
                // Found a cycle — mark all nodes on the stack path as recursive.
                // Conservative: mark both the current node and the back-edge target.
                recursive.insert(callee.clone());
                recursive.insert(node.to_string());
            } else if !visited.contains(callee.as_str()) {
                dfs_find_cycles(callee, callees, visited, on_stack, recursive);
            }
        }
    }

    on_stack.remove(node);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        BlockId, FunctionRef, LocalId, MirBlock, MirLocal, MirOperand, MirStmt, MirStmtKind,
        MirTerminator, MirTerminatorKind, MirType,
    };

    fn make_func(name: &str, calls: &[&str]) -> MirFunction {
        let mut stmts = Vec::new();
        for callee in calls {
            stmts.push(MirStmt::dummy(MirStmtKind::Call {
                dst: None,
                func: FunctionRef::internal(callee.to_string()),
                args: vec![],
            }));
        }
        MirFunction {
            name: name.to_string(),
            params: vec![],
            ret_ty: MirType::Void,
            locals: vec![],
            blocks: vec![MirBlock {
                id: BlockId(0),
                statements: stmts,
                terminator: MirTerminator::dummy(MirTerminatorKind::Return { value: None }),
            }],
            entry_block: BlockId(0),
            is_extern_c: false,
            source_file: None,
        }
    }

    #[test]
    fn call_count_tracks_sites() {
        let fns = vec![
            make_func("main", &["helper", "helper", "utils"]),
            make_func("helper", &[]),
            make_func("utils", &[]),
        ];
        let cg = CallGraph::build(&fns);
        assert_eq!(cg.call_count.get("helper"), Some(&2));
        assert_eq!(cg.call_count.get("utils"), Some(&1));
        assert!(cg.called_once("utils"));
        assert!(!cg.called_once("helper"));
    }

    #[test]
    fn detects_direct_recursion() {
        let fns = vec![make_func("factorial", &["factorial"])];
        let cg = CallGraph::build(&fns);
        assert!(cg.is_recursive("factorial"));
    }

    #[test]
    fn detects_mutual_recursion() {
        let fns = vec![
            make_func("is_even", &["is_odd"]),
            make_func("is_odd", &["is_even"]),
        ];
        let cg = CallGraph::build(&fns);
        assert!(cg.is_recursive("is_even"));
        assert!(cg.is_recursive("is_odd"));
    }

    #[test]
    fn non_recursive_not_flagged() {
        let fns = vec![
            make_func("main", &["helper"]),
            make_func("helper", &[]),
        ];
        let cg = CallGraph::build(&fns);
        assert!(!cg.is_recursive("main"));
        assert!(!cg.is_recursive("helper"));
    }

    #[test]
    fn statement_count() {
        let fns = vec![
            make_func("main", &["a", "b"]),
            make_func("a", &[]),
            make_func("b", &["a"]),
        ];
        let cg = CallGraph::build(&fns);
        assert_eq!(cg.statement_count("main"), 2);
        assert_eq!(cg.statement_count("a"), 0);
        assert_eq!(cg.statement_count("b"), 1);
    }

    #[test]
    fn extern_calls_ignored() {
        let mut fns = vec![make_func("main", &[])];
        // Manually add an extern call
        fns[0].blocks[0].statements.push(MirStmt::dummy(MirStmtKind::Call {
            dst: None,
            func: FunctionRef::extern_c("puts".to_string()),
            args: vec![],
        }));
        let cg = CallGraph::build(&fns);
        assert_eq!(cg.call_count.get("puts"), None);
    }
}
