"""tididi_bonsai.count and count_file: the answers, the errors, and calls from
several threads."""

import math
import subprocess
import sys
import threading
from fractions import Fraction

import pytest

import tididi_bonsai
from instances import (
    EXAMPLES,
    MALFORMED,
    PROJECTED_WEIGHTED,
    REFUTED,
    REPOSITORY,
    TWO_MODELS,
    TWO_MODELS_MC,
    WEIGHTED,
    clauses_of,
    parity_instances,
)

PARITY = parity_instances()


def test_a_model_count_is_an_exact_int():
    answer = tididi_bonsai.count(TWO_MODELS, mode="mc")
    assert (answer.mode, answer.satisfiable, answer.count) == ("mc", True, 2)
    assert type(answer.count) is int


def test_text_bytes_and_a_file_count_alike(tmp_path):
    path = tmp_path / "two.cnf"
    path.write_text(TWO_MODELS)
    for answer in (
        tididi_bonsai.count(TWO_MODELS.encode(), mode="mc"),
        tididi_bonsai.count_file(path, mode="mc"),
        tididi_bonsai.count_file(str(path), mode="mc"),
    ):
        assert (answer.mode, answer.count) == ("mc", 2)


def test_without_a_mode_the_header_chooses_and_no_header_compiles():
    assert tididi_bonsai.count(TWO_MODELS_MC).mode == "mc"
    answer = tididi_bonsai.count(TWO_MODELS)
    assert (answer.mode, answer.count) == ("compile", 2)


def test_a_refuted_formula_counts_zero_and_has_no_model():
    answer = tididi_bonsai.count(REFUTED, mode="mc")
    assert (answer.satisfiable, answer.count) == (False, 0)


def test_a_weighted_count_is_a_float():
    answer = tididi_bonsai.count(WEIGHTED)
    assert answer.mode == "wmc"
    assert type(answer.count) is float
    assert math.isclose(answer.count, 13 / 20, rel_tol=1e-9)


def test_a_projected_weighted_count_is_an_exact_fraction():
    answer = tididi_bonsai.count(PROJECTED_WEIGHTED)
    assert answer.mode == "pwmc"
    assert answer.count == Fraction(5, 12)
    assert type(answer.count) is Fraction


@pytest.mark.parametrize(("path", "mode"), PARITY, ids=[path.name for path, _ in PARITY])
def test_clauses_count_what_their_text_counts(path, mode):
    text = path.read_text()
    from_text = tididi_bonsai.count(text, mode=mode)
    from_clauses = tididi_bonsai.count(**clauses_of(text), mode=mode)
    assert (from_clauses.mode, from_clauses.satisfiable, from_clauses.count) == (
        from_text.mode,
        from_text.satisfiable,
        from_text.count,
    )


class PySatLikeCnf:
    """What a PySAT `CNF` offers: its clauses and its variable count."""

    def __init__(self, clauses, nv):
        self.clauses = clauses
        self.nv = nv


TWO_MODELS_CLAUSES = [[1, 2], [-1, 3], [-2, -3]]


def test_clauses_come_as_any_iterables_or_as_a_formula_object():
    assert tididi_bonsai.count(TWO_MODELS_CLAUSES, mode="mc").count == 2
    assert tididi_bonsai.count(((l for l in c) for c in TWO_MODELS_CLAUSES), mode="mc").count == 2
    assert tididi_bonsai.count(PySatLikeCnf(TWO_MODELS_CLAUSES, 3), mode="mc").count == 2


def test_num_vars_defaults_to_the_largest_variable_and_counts_free_ones():
    # A fourth variable no clause names doubles the count.
    assert tididi_bonsai.count(TWO_MODELS_CLAUSES, mode="mc", num_vars=4).count == 4
    assert tididi_bonsai.count(PySatLikeCnf(TWO_MODELS_CLAUSES, 4), mode="mc").count == 4
    assert tididi_bonsai.count(TWO_MODELS_CLAUSES, mode="pmc", show=[1, 4]).count == 4
    # Without a mode, clauses compile, and a show set makes the count projected.
    assert tididi_bonsai.count(TWO_MODELS_CLAUSES).mode == "compile"
    assert tididi_bonsai.count(TWO_MODELS_CLAUSES, show=[1]).mode == "pmc"


def test_weights_are_exact_whatever_their_python_type():
    # WEIGHTED's weights, each spelled another way.
    weights = {1: Fraction(3, 10), -1: "0.7", 2: 0.5, -2: "1/2"}
    answer = tididi_bonsai.count([[1, 2]], mode="wmc", weights=weights)
    assert answer.count == tididi_bonsai.count(WEIGHTED).count
    # x1 or x2, counted over x1, where x1 weighs 2 and then 0.1, and not x1
    # weighs 1: an int is exact, and a float is its shortest decimal.
    assert tididi_bonsai.count([[1, 2]], mode="pwmc", show=[1], weights={1: 2}).count == 3
    assert tididi_bonsai.count([[1, 2]], mode="pwmc", show=[1], weights={1: 0.1}).count == Fraction(11, 10)
    projected = {1: Fraction(1, 3), -1: Fraction(2, 3), 2: Fraction(1, 4), -2: Fraction(1, 2)}
    answer = tididi_bonsai.count([[-1, 3], [1, 2]], mode="pwmc", show=[1, 2], weights=projected)
    assert answer.count == Fraction(5, 12)
    # The README's example.
    weights = {1: "0.3", -1: "0.7", 2: 0.5}
    answer = tididi_bonsai.count(TWO_MODELS_CLAUSES, mode="pwmc", show=[1, 2], weights=weights)
    assert answer.count == Fraction(13, 20)


def test_clauses_that_name_no_variable_raise_input_error():
    with pytest.raises(tididi_bonsai.InputError, match="clause 2 holds 0"):
        tididi_bonsai.count([[1], [2, 0]], mode="mc")
    with pytest.raises(tididi_bonsai.InputError, match="names variable 1 twice"):
        tididi_bonsai.count([[1, -1]], mode="mc")
    with pytest.raises(tididi_bonsai.InputError, match="variable 3"):
        tididi_bonsai.count([[1, 3]], mode="mc", num_vars=2)
    with pytest.raises(tididi_bonsai.InputError, match="show variable 4"):
        tididi_bonsai.count([[1]], mode="pmc", num_vars=3, show=[4])
    with pytest.raises(tididi_bonsai.InputError, match="weight literal 0"):
        tididi_bonsai.count([[1]], mode="wmc", weights={0: 1})
    with pytest.raises(tididi_bonsai.InputError, match="the weight of literal 1"):
        tididi_bonsai.count([[1]], mode="wmc", weights={1: "heavy"})
    with pytest.raises(tididi_bonsai.InputError, match="the weight of literal 1"):
        tididi_bonsai.count([[1]], mode="wmc", weights={1: float("nan")})


def test_clause_keywords_are_refused_beside_dimacs_text():
    for keywords in ({"num_vars": 3}, {"show": [1]}, {"weights": {1: 1}}):
        with pytest.raises(tididi_bonsai.UsageError, match=next(iter(keywords))):
            tididi_bonsai.count(TWO_MODELS, mode="mc", **keywords)


def test_an_unweighted_count_refuses_weights():
    with pytest.raises(tididi_bonsai.UsageError, match="weight"):
        tididi_bonsai.count([[1, 2]], mode="mc", weights={1: "1/2"})


def test_clauses_and_weights_of_other_types_raise_type_error():
    with pytest.raises(TypeError, match="iterable"):
        tididi_bonsai.count([1, 2, 0], mode="mc")
    with pytest.raises(TypeError, match="weight of literal 1"):
        tididi_bonsai.count([[1]], mode="wmc", weights={1: [1]})
    with pytest.raises(TypeError, match="mapping"):
        tididi_bonsai.count([[1]], mode="wmc", weights=[(1, 1)])


def test_the_stats_record_carries_the_mode_and_the_count():
    stats = tididi_bonsai.count(TWO_MODELS, mode="mc").stats
    assert stats["mode"] == "mc"
    assert stats["model_count"] == "2"
    assert stats["exit_reason"] == "solved"


def test_malformed_dimacs_raises_input_error_naming_the_token():
    with pytest.raises(tididi_bonsai.InputError, match="x"):
        tididi_bonsai.count(MALFORMED, mode="mc")


def test_a_missing_file_raises_input_error_naming_it(tmp_path):
    with pytest.raises(tididi_bonsai.InputError, match="absent.cnf"):
        tididi_bonsai.count_file(tmp_path / "absent.cnf")


def test_an_unknown_mode_raises_usage_error_listing_the_modes():
    with pytest.raises(tididi_bonsai.UsageError, match="weighted") as raised:
        tididi_bonsai.count(TWO_MODELS, mode="weighted")
    assert "pwmc" in str(raised.value)


def test_a_mode_the_header_contradicts_raises_usage_error():
    with pytest.raises(tididi_bonsai.UsageError):
        tididi_bonsai.count(WEIGHTED, mode="mc")


def test_every_error_class_derives_from_error():
    for name in tididi_bonsai.__all__:
        value = getattr(tididi_bonsai, name)
        if name.endswith("Error") and name != "Error":
            assert issubclass(value, tididi_bonsai.Error), name
    assert issubclass(tididi_bonsai.Error, Exception)


def test_wrong_argument_types_raise_type_error():
    with pytest.raises(TypeError, match="str or bytes"):
        tididi_bonsai.count(42)
    with pytest.raises(TypeError):
        tididi_bonsai.count(TWO_MODELS, modes="mc")
    with pytest.raises(OverflowError):
        tididi_bonsai.count(TWO_MODELS, time_limit_ms=-1)


def test_an_answer_reprs_its_mode_and_count():
    assert repr(tididi_bonsai.count(TWO_MODELS, mode="mc")) == "<tididi_bonsai.Answer: mc, 2>"


def test_calls_from_several_threads_each_get_their_answer():
    answers = []

    def run():
        answers.append(tididi_bonsai.count(TWO_MODELS, mode="mc").count)

    threads = [threading.Thread(target=run) for _ in range(4)]
    for thread in threads:
        thread.start()
    for thread in threads:
        thread.join()
    assert answers == [2] * 4


def test_the_example_prints_what_it_promises():
    printed = subprocess.run(
        [sys.executable, str(EXAMPLES / "count.py"), str(REPOSITORY / "tests" / "data" / "pwmc_arjun_defined_weight.cnf")],
        capture_output=True,
        text=True,
        check=True,
        timeout=600,
    ).stdout
    assert printed == "mode: pwmc\nsatisfiable: True\ncount: 91/125\n"
