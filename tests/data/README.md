# Test data

The inputs the tests, the doctests and the bindings read.

- `*.cnf`: formulas written for this repository, by hand or by a generator.
- `*.vtree`: vtrees this compiler built, saved for the tests of formulas
  that once counted wrong on them.
- `cnfs/`: instances of the [Tractables/cnfs](https://github.com/Tractables/cnfs)
  corpus, unmodified apart from decompression, each at its path there. The
  `README.md` of each collection in the corpus gives the instances' source,
  license and citation; the Model Counting Competition instances under
  `cnfs/mcc/` are licensed CC BY 4.0.
- `protocol/`: what the binary prints on a fixed set of these, which
  `tests/protocol.rs` compares against.
