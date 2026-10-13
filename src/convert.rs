//! The one place vitri's values become tididi's, and back: a variable, a
//! clause literal, a vtree, and a weight table restated as the diagram
//! library's weighted-counting algebra.
//!
//! vitri and tididi each define a variable id, a literal and a vtree, and the
//! definitions agree field for field. They are still different types, because
//! the two crates cannot share them: tididi's `VarId`/`Literal` are the TDD
//! alphabet, defined in the pure `tididi` core that has no CNF concerns and no
//! native build, and making that core depend on vitri would put a CMake + Arjun
//! build in front of every `cargo test -p tididi`.
//!
//! The CNF formula is vitri's throughout the compiler, so nothing converts on
//! the way into a vitri call. What still crosses is the vtree, once per build
//! ([`vtree_from_vitri`]), the clause literals at the fold
//! ([`crate::compile::step`]), single variables wherever a clause or a
//! variable map meets the diagram ([`diagram_var`], [`formula_var`]), and the
//! weights a weighted compile counts under ([`rational_weight_algebra`]).

use num_rational::BigRational;
use tididi::vtree::{VarId, Vtree, VtreeIdx, VtreeNode};

/// One formula variable as the diagram's.
#[inline]
pub(crate) fn diagram_var(var: vitri::cnf::VarId) -> VarId {
    VarId(var.get())
}

/// One clause literal as the diagram's.
#[inline]
pub(crate) fn diagram_literal(lit: vitri::cnf::Literal) -> tididi::Literal {
    tididi::Literal { var: diagram_var(lit.var), sign: lit.positive }
}

/// One diagram variable as the formula's.
#[inline]
pub(crate) fn formula_var(var: VarId) -> vitri::cnf::VarId {
    vitri::cnf::VarId::new(var.0).expect("a diagram variable came from a formula variable, numbered from 1")
}

/// vitri's vtree as tididi's.
///
/// The node arrays agree in shape and in indexing, so this is a relabel of the
/// two index newtypes and nothing else; `from_nodes` recomputes the topological
/// order and leaf table the compiler navigates by.
pub(crate) fn vtree_from_vitri(vtree: &vitri::vtree::Vtree) -> Vtree {
    let nodes: Vec<VtreeNode> = (0..vtree.num_nodes() as u32)
        .map(|i| match vtree.node(vitri::vtree::VtreeIdx(i)) {
            vitri::vtree::VtreeNode::Leaf { var, parent } => VtreeNode::Leaf {
                var: diagram_var(*var),
                parent: parent.map(|p| VtreeIdx(p.0)),
            },
            vitri::vtree::VtreeNode::Internal {
                left,
                right,
                parent,
            } => VtreeNode::Internal {
                left: VtreeIdx(left.0),
                right: VtreeIdx(right.0),
                parent: parent.map(|p| VtreeIdx(p.0)),
            },
        })
        .collect();
    Vtree::from_nodes(nodes, VtreeIdx(vtree.root().0), vtree.num_vars())
        .expect("a vitri vtree relabels to one tree")
}

/// Convert `(negative, positive)` weight pairs — the order
/// [`vitri::cnf::WeightTable::resolve`] produces them in — to the library
/// algebra that weighted counting evaluates under.
pub(crate) fn rational_weight_algebra(weights: &[(BigRational, BigRational)]) -> tididi::diagram::RationalWeights {
    let (negative, positive) = weights.iter().cloned().unzip();
    tididi::diagram::RationalWeights::from_polarities(tididi::diagram::LiteralWeights { negative, positive })
        .expect("both polarity columns come from the same variable pairs")
}

#[cfg(test)]
#[path = "tests/convert.rs"]
mod tests;
