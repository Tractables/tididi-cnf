"""Count the models of a DIMACS CNF file, and print the mode the run resolved
to, whether the formula has a model, and the count.

    python count.py CNF [MODE]

MODE is compile, mc, wmc, pmc or pwmc; without it the instance's header
chooses. The run has a minute. For tests/data/pwmc_arjun_defined_weight.cnf,
a projected weighted count, it prints:

    mode: pwmc
    satisfiable: True
    count: 91/125
"""

import sys

import tididi_bonsai


def main(path, mode=None):
    try:
        answer = tididi_bonsai.count_file(path, mode=mode, time_limit_ms=60_000)
    except tididi_bonsai.Error as error:
        sys.exit(f"{type(error).__name__}: {error}")
    print(f"mode: {answer.mode}")
    print(f"satisfiable: {answer.satisfiable}")
    print(f"count: {answer.count}")


if __name__ == "__main__":
    if len(sys.argv) not in (2, 3):
        sys.exit(__doc__)
    main(*sys.argv[1:])
