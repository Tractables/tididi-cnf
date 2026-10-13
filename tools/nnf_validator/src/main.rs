//! Fast NNF model counter, metrics reporter, and format converter.
//!
//! Reads a d-DNNF circuit in any of four formats (standard NNF, D4, Panini, TDD),
//! counts models with arbitrary precision, reports structural metrics, and
//! optionally converts to standard NNF format.
//!
//! Supported input formats:
//!
//! 1. **Standard NNF** (c2d, DSHARP, GPMC, miniC2D, Bella, sharpSAT-TD):
//!    header `nnf N E V`, then `L lit`, `A c children...`, `O j c children...`.
//!
//! 2. **D4 format**: `o id 0` (OR), `a id 0` (AND), `t id 0` (true), `f id 0` (false),
//!    then edge lines `parent child lit1 lit2 ... 0`.
//!
//! 3. **Panini format**: `Number of nodes: N`, then indexed node lines
//!    `id:\tvar high low 0` (decision), `id:\tD children... 0` (AND),
//!    `id:\tT 0` / `id:\tF 0` (terminals).
//!
//! 4. **TDD format** (TiDiDi `.tdd` files): header `p tdd version num_vars num_vtree_nodes output_vtree output_local`,
//!    `L vtree_idx var` (vtree leaf), `I vtree_idx left_vtree right_vtree l0 r0 ...` (internal node).
//!    Model count is computed by reconstructing NNF from the TDD structure.
//!
//! Usage:
//!   nnf_validator <file.nnf> [--json] [--convert <output.nnf>]
//!
//! Output (default): the model count (one line, decimal integer)
//! Output (--json):  JSON object with model_count, nodes, edges, vars, etc.

use num_bigint::BigUint;
use num_traits::{One, Zero};
use rustc_hash::{FxHashMap, FxHashSet};
use smallvec::{smallvec, SmallVec};
use std::io::{BufWriter, Read, Write};

/// Child-index list for And/Or nodes. Almost every internal node has exactly
/// two children (TDD reconstruction emits 2-child And/Or pairs), so inlining
/// 2 elements avoids hundreds of millions of tiny heap allocations during the
/// parse/build phase on multi-GiB circuits. The `union` feature keeps this the
/// same size as a `Vec<usize>`, so node memory does not grow.
type Children = SmallVec<[usize; 2]>;
use std::{env, fs::File, process};

// ---------------------------------------------------------------------------
// Node representation (signed literals preserve polarity for NNF output)
// ---------------------------------------------------------------------------

enum Node {
    True,
    False,
    Literal(i32),
    And(Children),
    Or(Children),
}

// ---------------------------------------------------------------------------
// CLI arguments
// ---------------------------------------------------------------------------

struct Args {
    input: String,
    json: bool,
    convert: Option<String>,
}

fn parse_args() -> Args {
    let args: Vec<String> = env::args().collect();
    let mut input = None;
    let mut json = false;
    let mut convert = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--json" => json = true,
            "--convert" => {
                i += 1;
                if i >= args.len() {
                    eprintln!("Error: --convert requires a path");
                    process::exit(1);
                }
                convert = Some(args[i].clone());
            }
            "--help" | "-h" => {
                eprintln!(
                    "Usage: nnf_validator <file.nnf> [--json] [--convert <output.nnf>]"
                );
                process::exit(0);
            }
            arg if arg.starts_with('-') => {
                eprintln!("Error: unknown option '{arg}'");
                process::exit(1);
            }
            _ => {
                if input.is_some() {
                    eprintln!("Error: multiple input files not supported");
                    process::exit(1);
                }
                input = Some(args[i].clone());
            }
        }
        i += 1;
    }

    let input = input.unwrap_or_else(|| {
        eprintln!("Usage: nnf_validator <file.nnf> [--json] [--convert <output.nnf>]");
        process::exit(1);
    });

    Args {
        input,
        json,
        convert,
    }
}

// ---------------------------------------------------------------------------
// Standard NNF parser (c2d, DSHARP, GPMC, miniC2D, Bella, sharpSAT-TD)
// ---------------------------------------------------------------------------

fn parse_standard_nnf(lines: &[&str]) -> (Vec<Node>, usize, usize) {
    let mut nodes = Vec::new();
    let mut total_vars: usize = 0;
    let mut reported_vars: Option<usize> = None;

    // Pre-scan for solver metadata headers (before node parsing, which skips comments).
    //
    // - GPMC: "p vmap <orig> <simplified> <fixed> <free> <defined>" header.
    //   Fixed literals have 1 valid assignment, so mc_vars = orig - fixed.
    //   reported_vars = orig.
    //
    // - D4/sharpSAT-TD: "c Number of variables: N" (prepended by the harness).
    //   The solver omits preprocessed-away variables from the circuit.
    //   reported_vars = mc_vars = N (free-var shift handles the rest).
    for line in lines {
        let line = line.trim_start();
        if line.starts_with("p vmap ") {
            let parts: Vec<&str> = line.split_ascii_whitespace().collect();
            if parts.len() >= 5 {
                let orig: usize = parts[2].parse().unwrap_or(0);
                let fixed: usize = parts[4].parse().unwrap_or(0);
                reported_vars = Some(orig);
                total_vars = orig - fixed;
            }
            break;
        }
        if line.starts_with("c Number of variables:") {
            if let Some(n) = line.split(':').nth(1) {
                if let Ok(v) = n.trim().parse::<usize>() {
                    reported_vars = Some(v);
                    total_vars = v;
                }
            }
        }
    }

    for line in lines {
        let line = line.trim_start();
        if line.is_empty() || line.starts_with('c') {
            continue;
        }
        let bytes = line.as_bytes();
        match bytes[0] {
            b'n' => {
                let mut parts = line.split_ascii_whitespace();
                parts.next(); // "nnf"
                let n: usize = parts.next().unwrap().parse().unwrap();
                nodes.reserve(n);
                parts.next(); // edges
                let nnf_vars: usize = parts.next().unwrap().parse().unwrap();
                // Only override if no vmap header set a higher value
                if nnf_vars > total_vars {
                    total_vars = nnf_vars;
                }
            }
            b'L' => {
                let lit: i32 = line[2..].trim().parse().unwrap();
                nodes.push(Node::Literal(lit));
            }
            b'A' => {
                let mut iter = line[2..].split_ascii_whitespace();
                let c: usize = iter.next().unwrap().parse().unwrap();
                if c == 0 {
                    nodes.push(Node::True);
                } else {
                    let children: Children =
                        iter.take(c).map(|s| s.parse().unwrap()).collect();
                    nodes.push(Node::And(children));
                }
            }
            b'O' => {
                let mut iter = line[2..].split_ascii_whitespace();
                iter.next(); // decision var j
                let c: usize = iter.next().unwrap().parse().unwrap();
                if c == 0 {
                    nodes.push(Node::False);
                } else {
                    let children: Children =
                        iter.take(c).map(|s| s.parse().unwrap()).collect();
                    nodes.push(Node::Or(children));
                }
            }
            _ => {}
        }
    }

    (nodes, total_vars, reported_vars.unwrap_or(total_vars))
}

// ---------------------------------------------------------------------------
// D4 format parser
// ---------------------------------------------------------------------------

/// D4 format:
///   - Comment/header lines: `c ...`, `s <count>`
///   - Node declarations (1-indexed): `o <id> 0`, `a <id> 0`, `t <id> 0`, `f <id> 0`
///   - Edge lines: `<parent_id> <child_node> <lit1> <lit2> ... 0`
///
/// Each edge line defines an AND-branch from `parent_id`: the branch conjoins
/// the listed literals with the child node. D4 node IDs decrease from root to
/// leaves (root = node 1, highest ID = deepest leaf/true node).
fn parse_d4_format(lines: &[&str]) -> (Vec<Node>, usize, usize) {
    let mut max_node_id: usize = 0;
    let mut total_vars: usize = 0;

    // Extract total_vars from D4's comment header
    for line in lines {
        let line = line.trim();
        if let Some(rest) = line.strip_prefix("c Number of variables:") {
            if let Ok(v) = rest.trim().parse::<usize>() {
                total_vars = v;
            }
        }
    }

    struct D4Node {
        kind: char, // 'o', 't', 'f'
        /// Each branch: (child_node_id, vec of literal values)
        branches: Vec<(usize, Vec<i32>)>,
    }

    let mut d4_nodes: Vec<Option<D4Node>> = Vec::new();

    // First pass: collect node declarations
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('c') || line.starts_with('s') {
            continue;
        }
        let parts: Vec<&str> = line.split_ascii_whitespace().collect();
        if parts.len() >= 3 && (parts[0] == "o" || parts[0] == "a" || parts[0] == "t" || parts[0] == "f") {
            let id: usize = parts[1].parse().unwrap();
            if id > max_node_id {
                max_node_id = id;
            }
            if d4_nodes.len() <= id {
                d4_nodes.resize_with(id + 1, || None);
            }
            d4_nodes[id] = Some(D4Node {
                kind: parts[0].chars().next().unwrap(),
                branches: Vec::new(),
            });
        }
    }

    // Second pass: parse edge lines — format: parent child_node lit1 lit2 ... 0
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('c') || line.starts_with('s') {
            continue;
        }
        let bytes = line.as_bytes();
        if bytes[0] == b'o' || bytes[0] == b'a' || bytes[0] == b't' || bytes[0] == b'f' {
            if !bytes[0].is_ascii_digit() {
                continue; // node declaration, skip
            }
        }
        let parts: Vec<&str> = line.split_ascii_whitespace().collect();
        if parts.len() < 3 {
            continue;
        }
        let parent: usize = match parts[0].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        let child_node: usize = match parts[1].parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        // Remaining items before trailing 0 are literals
        let lits: Vec<i32> = parts[2..]
            .iter()
            .take_while(|&&s| s != "0")
            .map(|s| s.parse().unwrap())
            .collect();
        if parent < d4_nodes.len() {
            if let Some(ref mut node) = d4_nodes[parent] {
                node.branches.push((child_node, lits));
            }
        }
    }

    // Convert to our Node format via DFS from root (node 1).  D4 node IDs
    // do NOT follow a strict topological order — a non-terminal child can
    // have a lower ID than its parent (e.g., shared/cached components).
    // DFS ensures children are built before parents.
    let mut nodes: Vec<Node> = Vec::new();
    let mut id_to_idx: Vec<Option<usize>> = vec![None; max_node_id + 1];

    // Iterative DFS with explicit stack (avoids deep recursion)
    let mut stack: Vec<(usize, bool)> = vec![(1, false)]; // (id, children_done)
    while let Some((id, children_done)) = stack.pop() {
        if id >= d4_nodes.len() {
            continue;
        }
        if id_to_idx[id].is_some() {
            continue; // already built
        }
        let d4node = match &d4_nodes[id] {
            Some(n) => n,
            None => continue,
        };

        match d4node.kind {
            'f' => {
                id_to_idx[id] = Some(nodes.len());
                nodes.push(Node::False);
            }
            't' => {
                id_to_idx[id] = Some(nodes.len());
                nodes.push(Node::True);
            }
            'a' | 'o' => {
                if !children_done {
                    // Push self back with children_done=true, then push children
                    stack.push((id, true));
                    for (child_id, _) in d4node.branches.iter().rev() {
                        if *child_id < id_to_idx.len() && id_to_idx[*child_id].is_none() {
                            stack.push((*child_id, false));
                        }
                    }
                    continue;
                }
                // Children are built — now build this node
                if d4node.kind == 'a' {
                    // AND node: conjunction of all branches (child + literals)
                    let mut and_children: Children = Children::new();
                    for (child_id, lits) in &d4node.branches {
                        for &lit in lits {
                            and_children.push(nodes.len());
                            nodes.push(Node::Literal(lit));
                        }
                        if *child_id < id_to_idx.len() {
                            if let Some(child_idx) = id_to_idx[*child_id] {
                                and_children.push(child_idx);
                            }
                        }
                    }
                    id_to_idx[id] = Some(nodes.len());
                    if and_children.is_empty() {
                        nodes.push(Node::True);
                    } else {
                        nodes.push(Node::And(and_children));
                    }
                } else {
                    // OR node
                    if d4node.branches.is_empty() {
                        id_to_idx[id] = Some(nodes.len());
                        nodes.push(Node::False);
                    } else {
                        let mut or_children: Children = Children::new();
                        for (child_id, lits) in &d4node.branches {
                            let mut and_children: Children = Children::new();
                            for &lit in lits {
                                and_children.push(nodes.len());
                                nodes.push(Node::Literal(lit));
                            }
                            if *child_id < id_to_idx.len() {
                                if let Some(child_idx) = id_to_idx[*child_id] {
                                    and_children.push(child_idx);
                                }
                            }
                            let and_idx = nodes.len();
                            if and_children.is_empty() {
                                nodes.push(Node::True);
                            } else {
                                nodes.push(Node::And(and_children));
                            }
                            or_children.push(and_idx);
                        }
                        id_to_idx[id] = Some(nodes.len());
                        nodes.push(Node::Or(or_children));
                    }
                }
            }
            _ => {}
        }
    }

    (nodes, total_vars, total_vars)
}

// ---------------------------------------------------------------------------
// TDD format parser (TiDiDi .tdd files)
// ---------------------------------------------------------------------------

/// TiDiDi TDD format:
/// ```text
/// c TiDiDi TDD circuit
/// p tdd <version> <num_vars> <num_vtree_nodes> <output_vtree> <output_local>
/// L <vtree_idx> <var>                                    -- vtree leaf
/// I <vtree_idx> <left_vtree> <right_vtree> <l0> <r0> ... -- TDD internal node
/// ```
///
/// Each vtree leaf has 3 implicit TDD nodes: one(0), pos(1), neg(2).
///
/// Conversion to NNF:
/// - one(v) → O v 2 (L v) (L -v)  (model count = 2)
/// - pos(v) → L v        (model count = 1)
/// - neg(v) → L -v       (model count = 1)
/// - internal with k pairs → O 0 k (A 2 left right) ...
fn parse_tdd_format(lines: &[&str]) -> (Vec<Node>, usize, usize) {
    let mut num_vars: usize = 0;
    let mut output_vtree: usize = 0;
    let mut output_local: usize = 0;
    let mut is_zero = false;

    // Vtree leaf: vtree_idx → 1-indexed DIMACS variable
    let mut vtree_var: FxHashMap<usize, i32> = FxHashMap::default();
    // Vtree internal: vtree_idx → (left_vtree, right_vtree)
    let mut vtree_children: FxHashMap<usize, (usize, usize)> = FxHashMap::default();

    // TDD internal nodes per vtree level: vtree_idx → vec of (pairs)
    // Each entry is one TDD node; local_idx = position in vec.
    let mut internal_nodes: FxHashMap<usize, Vec<Vec<(usize, usize)>>> = FxHashMap::default();

    // Parse
    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('c') {
            continue;
        }
        let parts: Vec<&str> = line.split_ascii_whitespace().collect();
        if parts.is_empty() {
            continue;
        }
        match parts[0] {
            // The version is parts[2]; every field after it shifts by one.
            "p" if parts.len() >= 7 && parts[1] == "tdd" => {
                num_vars = parts[3].parse().unwrap_or(0);
                output_vtree = parts[5].parse().unwrap_or(0);
                if parts[6] == "ZERO" {
                    is_zero = true;
                } else {
                    output_local = parts[6].parse().unwrap_or(0);
                }
            }
            "L" if parts.len() >= 3 => {
                let vi: usize = parts[1].parse().unwrap();
                let var: i32 = parts[2].parse().unwrap();
                vtree_var.insert(vi, var);
            }
            "I" if parts.len() >= 5 => {
                let vi: usize = parts[1].parse().unwrap();
                let left_v: usize = parts[2].parse().unwrap();
                let right_v: usize = parts[3].parse().unwrap();
                vtree_children.entry(vi).or_insert((left_v, right_v));
                // Remaining tokens are left/right pairs
                let mut pairs = Vec::new();
                let pair_data = &parts[4..];
                for chunk in pair_data.chunks(2) {
                    if chunk.len() == 2 {
                        let l: usize = chunk[0].parse().unwrap();
                        let r: usize = chunk[1].parse().unwrap();
                        pairs.push((l, r));
                    }
                }
                internal_nodes.entry(vi).or_default().push(pairs);
            }
            _ => {}
        }
    }

    if is_zero {
        let nodes = vec![Node::False];
        return (nodes, num_vars, num_vars);
    }

    // Build NNF nodes bottom-up via iterative vtree traversal.
    // nnf_idx[(vtree_idx, local_idx)] = NNF node index for that TDD node.
    let mut nnf_idx: FxHashMap<(usize, usize), usize> = FxHashMap::default();
    let mut nodes: Vec<Node> = Vec::new();

    // Iterative post-order traversal (avoids stack overflow on deep vtrees).
    let mut stack: Vec<(usize, bool)> = vec![(output_vtree, false)];
    while let Some((vi, children_done)) = stack.pop() {
        if let Some(&var) = vtree_var.get(&vi) {
            // Leaf vtree node: 3 implicit TDD nodes (one=0, pos=1, neg=2)
            let one_pos = nodes.len();
            nodes.push(Node::Literal(var));
            let one_neg = nodes.len();
            nodes.push(Node::Literal(-var));
            let one_idx = nodes.len();
            nodes.push(Node::Or(smallvec![one_pos, one_neg]));
            nnf_idx.insert((vi, 0), one_idx);

            let pos_idx = nodes.len();
            nodes.push(Node::Literal(var));
            nnf_idx.insert((vi, 1), pos_idx);

            let neg_idx = nodes.len();
            nodes.push(Node::Literal(-var));
            nnf_idx.insert((vi, 2), neg_idx);
        } else if let Some(&(left, right)) = vtree_children.get(&vi) {
            if !children_done {
                stack.push((vi, true));
                stack.push((right, false));
                stack.push((left, false));
                continue;
            }
            // Children are built — build TDD internal nodes at this level.
            if let Some(node_list) = internal_nodes.get(&vi) {
                for (local_idx, pairs) in node_list.iter().enumerate() {
                    if pairs.is_empty() {
                        let idx = nodes.len();
                        nodes.push(Node::False);
                        nnf_idx.insert((vi, local_idx), idx);
                        continue;
                    }
                    let mut or_children: Children = SmallVec::with_capacity(pairs.len());
                    for &(l, r) in pairs {
                        let left_nnf = *nnf_idx.get(&(left, l)).unwrap_or_else(|| {
                            panic!("missing nnf_idx for left child ({}, {}) at vtree {} (right={})", left, l, vi, right);
                        });
                        let right_nnf = *nnf_idx.get(&(right, r)).unwrap_or_else(|| {
                            panic!("missing nnf_idx for right child ({}, {}) at vtree {} (left={})", right, r, vi, left);
                        });
                        let and_idx = nodes.len();
                        nodes.push(Node::And(smallvec![left_nnf, right_nnf]));
                        or_children.push(and_idx);
                    }
                    if or_children.len() == 1 {
                        nnf_idx.insert((vi, local_idx), or_children[0]);
                    } else {
                        let or_idx = nodes.len();
                        nodes.push(Node::Or(or_children));
                        nnf_idx.insert((vi, local_idx), or_idx);
                    }
                }
            }
        }
    }

    // The output node
    let root_nnf = nnf_idx[&(output_vtree, output_local)];

    // Ensure root is last node (expected by model counter)
    if root_nnf != nodes.len() - 1 {
        nodes.push(Node::And(smallvec![root_nnf]));
    }

    (nodes, num_vars, num_vars)
}


// ---------------------------------------------------------------------------
// Panini format parser
// ---------------------------------------------------------------------------

/// Panini/KCBox Decision-DNNF format:
///   Number of nodes: N
///   0:\tF 0           — false terminal
///   1:\tT 0           — true terminal
///   2:\t<var> <hi> <lo> 0   — decision node (OR over positive/negative branches)
///   N:\tD <c1> <c2> ... 0   — AND/decomposition node
///
/// Decision nodes are expanded into: OR(AND(+var, hi), AND(-var, lo))
fn parse_panini_format(lines: &[&str]) -> (Vec<Node>, usize, usize) {
    // First pass: determine node count and collect raw node data
    struct RawNode {
        kind: char,       // 'F', 'T', 'D', or 'V' (decision on variable)
        var: i32,         // variable for decision nodes
        children: Vec<usize>, // hi,lo for decision; children for AND
    }

    let mut raw_nodes: Vec<Option<RawNode>> = Vec::new();
    let mut max_id: usize = 0;

    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with("Number of nodes:") || line.starts_with('c') {
            continue;
        }
        // Parse "id:\t..." or "id:  ..."
        let colon_pos = match line.find(':') {
            Some(p) => p,
            None => continue,
        };
        let id: usize = match line[..colon_pos].trim().parse() {
            Ok(v) => v,
            Err(_) => continue,
        };
        if id > max_id {
            max_id = id;
        }
        let rest = line[colon_pos + 1..].trim();
        let tokens: Vec<&str> = rest.split_ascii_whitespace().collect();
        if tokens.is_empty() {
            continue;
        }

        let raw = if tokens[0] == "F" {
            RawNode { kind: 'F', var: 0, children: vec![] }
        } else if tokens[0] == "T" {
            RawNode { kind: 'T', var: 0, children: vec![] }
        } else if tokens[0] == "D" {
            // AND node: D <c1> <c2> ... 0
            let children: Vec<usize> = tokens[1..]
                .iter()
                .take_while(|&&s| s != "0")
                .map(|s| s.parse().unwrap())
                .collect();
            RawNode { kind: 'D', var: 0, children }
        } else {
            // Decision node: <var> <hi> <lo> 0
            let var: i32 = match tokens[0].parse() {
                Ok(v) => v,
                Err(_) => continue,
            };
            let hi: usize = tokens[1].parse().unwrap();
            let lo: usize = tokens[2].parse().unwrap();
            RawNode { kind: 'V', var, children: vec![hi, lo] }
        };

        if raw_nodes.len() <= id {
            raw_nodes.resize_with(id + 1, || None);
        }
        raw_nodes[id] = Some(raw);
    }

    // Second pass: build Node vector. Each Panini node maps to one or more
    // internal nodes. Decision nodes expand into 5 nodes:
    //   L(+var), L(-var), AND(+var, hi), AND(-var, lo), OR(and_hi, and_lo)
    //
    // We need to process in an order where children are built before parents.
    // Panini typically numbers nodes bottom-up (leaves first), but decision
    // node children can reference higher IDs (forward references). Use a
    // recursive approach with memoization.

    let mut nodes: Vec<Node> = Vec::new();
    let mut id_to_idx: Vec<Option<usize>> = vec![None; max_id + 1];

    // Iterative topological build: process nodes from id 0 upward.
    // Panini numbers nodes such that children have lower IDs than parents.
    for id in 0..=max_id {
        if id >= raw_nodes.len() {
            continue;
        }
        let raw = match &raw_nodes[id] {
            Some(r) => r,
            None => continue,
        };

        match raw.kind {
            'F' => {
                id_to_idx[id] = Some(nodes.len());
                nodes.push(Node::False);
            }
            'T' => {
                id_to_idx[id] = Some(nodes.len());
                nodes.push(Node::True);
            }
            'D' => {
                // AND node
                let children: Children = raw.children.iter()
                    .filter_map(|&c| id_to_idx[c])
                    .collect();
                id_to_idx[id] = Some(nodes.len());
                if children.is_empty() {
                    nodes.push(Node::True);
                } else {
                    nodes.push(Node::And(children));
                }
            }
            'V' => {
                // Decision node: expand to OR(AND(+var, hi), AND(-var, lo))
                let var = raw.var;
                let hi = raw.children[0];
                let lo = raw.children[1];

                let pos_lit_idx = nodes.len();
                nodes.push(Node::Literal(var));

                let neg_lit_idx = nodes.len();
                nodes.push(Node::Literal(-var));

                let hi_child = id_to_idx[hi];
                let and_hi_idx = nodes.len();
                match hi_child {
                    Some(hi_idx) => nodes.push(Node::And(smallvec![pos_lit_idx, hi_idx])),
                    None => nodes.push(Node::And(smallvec![pos_lit_idx])),
                }

                let lo_child = id_to_idx[lo];
                let and_lo_idx = nodes.len();
                match lo_child {
                    Some(lo_idx) => nodes.push(Node::And(smallvec![neg_lit_idx, lo_idx])),
                    None => nodes.push(Node::And(smallvec![neg_lit_idx])),
                }

                id_to_idx[id] = Some(nodes.len());
                nodes.push(Node::Or(smallvec![and_hi_idx, and_lo_idx]));
            }
            _ => {}
        }
    }

    // Determine total_vars from literals seen
    let mut vars_seen = FxHashSet::default();
    for node in &nodes {
        if let Node::Literal(lit) = node {
            vars_seen.insert(lit.unsigned_abs());
        }
    }
    let v = vars_seen.len();
    (nodes, v, v)
}

// ---------------------------------------------------------------------------
// Format detection and dispatch
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Format {
    Standard,
    Tdd,
    Panini,
    D4,
}

/// Detect the circuit format by scanning for characteristic headers.
/// Mirrors the original detect-and-parse dispatch order.
fn detect_format(lines: &[&str]) -> Format {
    if lines.iter().any(|l| l.trim_start().starts_with("nnf ")) {
        return Format::Standard;
    }
    if lines.iter().any(|l| l.trim_start().starts_with("p tdd ")) {
        return Format::Tdd;
    }
    if lines.iter().any(|l| l.trim().starts_with("Number of nodes:")) {
        return Format::Panini;
    }
    Format::D4
}

/// Parse `lines` into a Node DAG. Returns (nodes, mc_vars, reported_vars).
/// - mc_vars: variable count for model counting (excludes fixed literals).
/// - reported_vars: original variable count for JSON output / validation.
/// For most formats these are equal; GPMC's vmap header makes them differ.
fn parse_to_nodes(lines: &[&str], fmt: Format) -> (Vec<Node>, usize, usize) {
    match fmt {
        Format::Standard => parse_standard_nnf(lines),
        Format::Tdd => parse_tdd_format(lines),
        Format::Panini => parse_panini_format(lines),
        Format::D4 => parse_d4_format(lines),
    }
}

/// Streaming model counter for the TDD format.
///
/// `.tdd` emits all leaf (`L`) lines first, then internal (`I`) lines in
/// bottom-up vtree order — every node's children-level appears before the node
/// itself (see `src/tdd/save.rs::write_tdd`). This lets us count without ever
/// materialising the ~hundreds-of-millions-of-node reconstructed NNF DAG: we
/// keep only one `BigUint` model count per TDD node, grouped by vtree level,
/// and free a level's counts the moment its parent level's block completes.
///
/// Correctness: the TDD is smooth, so every node at a given vtree level shares
/// the same variable scope. Hence no smoothing shifts are needed —
///   - leaf:     one=2, pos=1, neg=1
///   - internal: count = Σ over pairs (l,r) of  count_left[l] · count_right[r]
///   - final:    root_count << (num_vars − scope(output_vtree))
/// This yields a model count identical to the Node-DAG path's `count_models`,
/// but at a small fraction of the peak memory.
///
/// The returned `Metrics` mirror `collect_metrics` on the equivalent NNF
/// (minus the at-most-one root-wrapper node). The benchmark harness consumes
/// only `model_count`; the structural metrics are a faithful estimate for
/// standalone `--json` inspection.
fn count_tdd_streaming(lines: &[&str]) -> (BigUint, Metrics) {
    let mut num_vars: usize = 0;
    let mut output_vtree: usize = 0;
    let mut output_local: usize = 0;
    let mut is_zero = false;

    // Per vtree level: model counts of that level's TDD nodes, indexed by local
    // idx. Freed as soon as the level's parent block completes.
    let mut counts: FxHashMap<usize, Vec<BigUint>> = FxHashMap::default();
    // Per vtree level: number of leaf variables in that level's (uniform) scope.
    let mut scope: FxHashMap<usize, usize> = FxHashMap::default();
    // Per internal vtree level: its (left, right) child vtree indices, for freeing.
    let mut level_children: FxHashMap<usize, (usize, usize)> = FxHashMap::default();

    // Metrics accumulators (mirror collect_metrics on the equivalent NNF).
    let mut m_nodes = 0usize;
    let mut m_edges = 0usize;
    let mut m_and = 0usize;
    let mut m_or = 0usize;
    let mut m_leaf = 0usize;
    let mut n_vars = 0usize; // distinct leaf vars = number of L lines

    let mut cur_level = usize::MAX; // vtree level whose I-block we're reading

    for line in lines {
        let line = line.trim();
        if line.is_empty() || line.starts_with('c') {
            continue;
        }
        let bytes = line.as_bytes();
        match bytes[0] {
            b'p' => {
                let parts: Vec<&str> = line.split_ascii_whitespace().collect();
                // parts[2] is the format version; the fields after it shift by one.
                if parts.len() >= 7 && parts[1] == "tdd" {
                    num_vars = parts[3].parse().unwrap_or(0);
                    output_vtree = parts[5].parse().unwrap_or(0);
                    if parts[6] == "ZERO" {
                        is_zero = true;
                    } else {
                        output_local = parts[6].parse().unwrap_or(0);
                    }
                }
            }
            b'L' => {
                // L vtree_idx var — leaf level with 3 implicit nodes one/pos/neg.
                let mut it = line.split_ascii_whitespace();
                it.next(); // "L"
                let vi: usize = it.next().unwrap().parse().unwrap();
                counts.insert(
                    vi,
                    vec![BigUint::from(2u32), BigUint::one(), BigUint::one()],
                );
                scope.insert(vi, 1);
                n_vars += 1;
                // NNF: 4 literals + 1 Or (one node).
                m_nodes += 5;
                m_leaf += 4;
                m_or += 1;
                m_edges += 2;
            }
            b'I' => {
                // I vtree_idx left_vtree right_vtree l0 r0 l1 r1 ...
                let mut it = line.split_ascii_whitespace();
                it.next(); // "I"
                let vi: usize = it.next().unwrap().parse().unwrap();
                let left: usize = it.next().unwrap().parse().unwrap();
                let right: usize = it.next().unwrap().parse().unwrap();

                if vi != cur_level {
                    // New level's block begins: the previous level is complete,
                    // so its children's counts can never be referenced again.
                    if cur_level != usize::MAX {
                        if let Some(&(l, r)) = level_children.get(&cur_level) {
                            counts.remove(&l);
                            counts.remove(&r);
                        }
                    }
                    cur_level = vi;
                    let sl = *scope.get(&left).expect("left child scope must exist");
                    let sr = *scope.get(&right).expect("right child scope must exist");
                    scope.insert(vi, sl + sr);
                    level_children.insert(vi, (left, right));
                    counts.entry(vi).or_default();
                }

                let lc = counts.get(&left).expect("left child counts must exist");
                let rc = counts.get(&right).expect("right child counts must exist");
                let mut node_count = BigUint::zero();
                let mut n_pairs = 0usize;
                loop {
                    let l = match it.next() {
                        Some(s) => s,
                        None => break,
                    };
                    let r = it.next().expect("dangling pair element in I line");
                    let li: usize = l.parse().unwrap();
                    let ri: usize = r.parse().unwrap();
                    node_count += &lc[li] * &rc[ri];
                    n_pairs += 1;
                }
                if n_pairs == 0 {
                    m_nodes += 1; // False
                    m_leaf += 1;
                } else {
                    m_nodes += n_pairs; // And per pair
                    m_and += n_pairs;
                    m_edges += 2 * n_pairs;
                    if n_pairs > 1 {
                        m_nodes += 1; // Or over the Ands
                        m_or += 1;
                        m_edges += n_pairs;
                    }
                }
                // lc/rc borrows end above; safe to mutably push (vi != left/right).
                counts.get_mut(&vi).unwrap().push(node_count);
            }
            _ => {}
        }
    }

    if is_zero {
        let metrics = Metrics {
            nodes: 1,
            edges: 0,
            vars: num_vars,
            and_nodes: 0,
            or_nodes: 0,
            leaf_nodes: 1,
        };
        return (BigUint::zero(), metrics);
    }

    let root = counts
        .get(&output_vtree)
        .and_then(|v| v.get(output_local))
        .cloned()
        .unwrap_or_else(BigUint::zero);
    let root_scope = *scope.get(&output_vtree).unwrap_or(&num_vars);
    let free_vars = num_vars.saturating_sub(root_scope);
    let mc = if free_vars == 0 { root } else { root << free_vars };

    let metrics = Metrics {
        nodes: m_nodes,
        edges: m_edges,
        vars: n_vars,
        and_nodes: m_and,
        or_nodes: m_or,
        leaf_nodes: m_leaf,
    };
    (mc, metrics)
}

// ---------------------------------------------------------------------------
// Metrics collection
// ---------------------------------------------------------------------------

struct Metrics {
    nodes: usize,
    edges: usize,
    vars: usize,
    and_nodes: usize,
    or_nodes: usize,
    leaf_nodes: usize,
}

fn collect_metrics(nodes: &[Node]) -> Metrics {
    let mut edges = 0usize;
    let mut and_nodes = 0usize;
    let mut or_nodes = 0usize;
    let mut leaf_nodes = 0usize;
    let mut vars_seen = FxHashSet::default();

    for node in nodes {
        match node {
            Node::True | Node::False => leaf_nodes += 1,
            Node::Literal(lit) => {
                leaf_nodes += 1;
                vars_seen.insert(lit.unsigned_abs());
            }
            Node::And(children) => {
                and_nodes += 1;
                edges += children.len();
            }
            Node::Or(children) => {
                or_nodes += 1;
                edges += children.len();
            }
        }
    }

    Metrics {
        nodes: nodes.len(),
        edges,
        vars: vars_seen.len(),
        and_nodes,
        or_nodes,
        leaf_nodes,
    }
}

// ---------------------------------------------------------------------------
// Standard NNF writer
// ---------------------------------------------------------------------------

fn write_standard_nnf(nodes: &[Node], total_vars: usize, path: &str) -> std::io::Result<()> {
    let file = File::create(path)?;
    let mut w = BufWriter::new(file);

    // Count edges for header
    let mut edge_count = 0usize;
    for node in nodes {
        match node {
            Node::And(c) | Node::Or(c) => edge_count += c.len(),
            _ => {}
        }
    }

    writeln!(w, "nnf {} {} {}", nodes.len(), edge_count, total_vars)?;

    for node in nodes {
        match node {
            Node::True => writeln!(w, "A 0")?,
            Node::False => writeln!(w, "O 0 0")?,
            Node::Literal(lit) => writeln!(w, "L {lit}")?,
            Node::And(children) => {
                write!(w, "A {}", children.len())?;
                for &c in children {
                    write!(w, " {c}")?;
                }
                writeln!(w)?;
            }
            Node::Or(children) => {
                write!(w, "O 0 {}", children.len())?;
                for &c in children {
                    write!(w, " {c}")?;
                }
                writeln!(w)?;
            }
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Model counting
// ---------------------------------------------------------------------------

fn count_models(nodes: &[Node], total_vars: usize) -> BigUint {
    if nodes.is_empty() {
        return BigUint::zero();
    }

    let mut counts: Vec<BigUint> = Vec::with_capacity(nodes.len());
    let mut scope_sizes: Vec<usize> = Vec::with_capacity(nodes.len());

    for node in nodes {
        match node {
            Node::True => {
                counts.push(BigUint::one());
                scope_sizes.push(0);
            }
            Node::False => {
                counts.push(BigUint::zero());
                scope_sizes.push(0);
            }
            Node::Literal(_) => {
                counts.push(BigUint::one());
                scope_sizes.push(1);
            }
            Node::And(children) => {
                let mut prod = BigUint::one();
                let mut svars = 0usize;
                for &c in children {
                    prod *= &counts[c];
                    svars += scope_sizes[c];
                }
                counts.push(prod);
                scope_sizes.push(svars);
            }
            Node::Or(children) => {
                let max_scope = children.iter().map(|&c| scope_sizes[c]).max().unwrap_or(0);
                let mut sum = BigUint::zero();
                for &c in children {
                    let gap = max_scope - scope_sizes[c];
                    if counts[c].is_zero() {
                        continue;
                    }
                    if gap == 0 {
                        sum += &counts[c];
                    } else {
                        sum += &counts[c] << gap;
                    }
                }
                counts.push(sum);
                scope_sizes.push(max_scope);
            }
        }
    }

    let root_count = counts.last().unwrap();
    let root_scope = *scope_sizes.last().unwrap();
    let free_vars = total_vars.saturating_sub(root_scope);

    if free_vars == 0 {
        root_count.clone()
    } else {
        root_count << free_vars
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

/// Emit the result on stdout, in `--json` or plain form.
fn emit(args: &Args, model_count: &BigUint, metrics: &Metrics, reported_vars: usize) {
    if args.json {
        println!(
            "{{\"model_count\":\"{model_count}\",\"nodes\":{},\"edges\":{},\"vars\":{},\"solver_declared_vars\":{reported_vars},\"and_nodes\":{},\"or_nodes\":{},\"leaf_nodes\":{}}}",
            metrics.nodes, metrics.edges, metrics.vars, metrics.and_nodes, metrics.or_nodes, metrics.leaf_nodes
        );
    } else {
        println!("{model_count}");
    }
}

fn main() {
    let args = parse_args();

    let mut file = File::open(&args.input).unwrap_or_else(|e| {
        eprintln!("Error: cannot open {}: {e}", args.input);
        process::exit(1);
    });

    // Slurp the whole file into one buffer and borrow line slices from it,
    // rather than allocating a String per line. On multi-GiB .tdd circuits
    // (tens of millions of lines) this avoids tens of millions of small heap
    // allocations and is a large parse-time win.
    let timing = std::env::var_os("NNF_VALIDATOR_TIMING").is_some();
    let t0 = std::time::Instant::now();
    let mut buf = String::new();
    file.read_to_string(&mut buf).unwrap_or_else(|e| {
        eprintln!("Error: cannot read {}: {e}", args.input);
        process::exit(1);
    });
    let lines: Vec<&str> = buf.lines().collect();
    let fmt = detect_format(&lines);

    // Fast path: TDD format without --convert is counted by streaming over the
    // bottom-up node listing, never building the reconstructed NNF DAG. This is
    // the memory-critical benchmark-validation path.
    // Escape hatch: NNF_VALIDATOR_FORCE_DAG=1 routes TDD through the reference
    // Node-DAG path (parse_tdd_format + count_models) instead of the streaming
    // counter, for differential debugging if a streaming count is ever doubted.
    let force_dag = std::env::var_os("NNF_VALIDATOR_FORCE_DAG").is_some();
    if fmt == Format::Tdd && args.convert.is_none() && !force_dag {
        let t_read = t0.elapsed();
        let t1 = std::time::Instant::now();
        let (model_count, metrics) = count_tdd_streaming(&lines);
        if timing {
            eprintln!(
                "[timing] streaming read={:.2}s count={:.2}s nodes={}",
                t_read.as_secs_f64(),
                t1.elapsed().as_secs_f64(),
                metrics.nodes
            );
        }
        emit(&args, &model_count, &metrics, metrics.vars);
        return;
    }

    let (nodes, mc_vars, reported_vars) = parse_to_nodes(&lines, fmt);
    let t_parse = t0.elapsed();

    // Optionally write standard NNF (uses mc_vars for the header)
    if let Some(ref path) = args.convert {
        if let Err(e) = write_standard_nnf(&nodes, mc_vars, path) {
            eprintln!("Error: cannot write {path}: {e}");
            process::exit(1);
        }
    }

    let metrics = collect_metrics(&nodes);
    let t1 = std::time::Instant::now();
    let model_count = count_models(&nodes, mc_vars);
    if timing {
        eprintln!(
            "[timing] nodes={} parse={:.2}s count={:.2}s",
            nodes.len(),
            t_parse.as_secs_f64(),
            t1.elapsed().as_secs_f64()
        );
    }

    emit(&args, &model_count, &metrics, reported_vars);
}
