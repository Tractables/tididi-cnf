"""tididi_bonsai counts what `tididi --mode MODE` counts, on the instances
bindings/parity.txt lists. Runs when pytest is given --tididi-cli."""

import math
import subprocess
from fractions import Fraction

import pytest

import tididi_bonsai
from instances import parity_instances

INSTANCES = parity_instances()


def answered_by_the_command_line(cli, path, mode):
    """The verdict and the count `tididi` prints in the competition's format:
    its status line, and the last word of its `c s exact` line."""
    output = subprocess.run(
        [str(cli), "--mode", mode, "--output-format", "mcc", "--mem-limit-mb", "0", str(path)],
        capture_output=True,
        text=True,
        check=True,
        timeout=600,
    ).stdout.splitlines()
    status = next(line for line in output if line.startswith("s "))
    exact = next(line for line in output if line.startswith("c s exact "))
    return status == "s SATISFIABLE", exact.split()[-1]


def same_count(count, text):
    if isinstance(count, float):
        return math.isclose(count, float(text), rel_tol=1e-12, abs_tol=0.0)
    if isinstance(count, Fraction):
        return f"{count.numerator}/{count.denominator}" == text
    return str(count) == text


def test_the_list_names_instances_that_exist():
    assert INSTANCES
    for path, _ in INSTANCES:
        assert path.is_file(), path


@pytest.mark.parametrize(("path", "mode"), INSTANCES, ids=[path.name for path, _ in INSTANCES])
def test_count_matches_the_command_line(tididi_cli, path, mode):
    satisfiable, count = answered_by_the_command_line(tididi_cli, path, mode)
    for answer in (
        tididi_bonsai.count_file(path, mode=mode),
        tididi_bonsai.count(path.read_bytes(), mode=mode),
    ):
        assert answer.mode == mode
        assert answer.satisfiable == satisfiable
        assert same_count(answer.count, count), (answer.count, count)
