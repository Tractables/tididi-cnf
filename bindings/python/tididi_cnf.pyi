import os
from collections.abc import Iterable, Mapping
from fractions import Fraction
from typing import Any, Protocol

class _Clauses(Protocol):
    """A formula object such as a PySAT `CNF`: its clauses, and `nv`, the
    variable count, if it has one."""

    @property
    def clauses(self) -> Iterable[Iterable[int]]: ...

__version__: str

class Error(Exception): ...
class UsageError(Error): ...
class EnvError(Error): ...
class StageError(Error): ...
class VtreeSpecError(Error): ...
class InputError(Error): ...
class ArtifactError(Error): ...
class IncompleteError(Error): ...

class Answer:
    @property
    def mode(self) -> str: ...
    @property
    def satisfiable(self) -> bool: ...
    @property
    def count(self) -> int | Fraction | float: ...
    @property
    def stats(self) -> dict[str, Any]: ...

def count(
    formula: str | bytes | Iterable[Iterable[int]] | _Clauses,
    *,
    mode: str | None = None,
    num_vars: int | None = None,
    show: Iterable[int] | None = None,
    weights: Mapping[int, int | float | Fraction | str] | None = None,
    time_limit_ms: int | None = None,
    mem_limit_mb: int = 0,
) -> Answer: ...
def count_file(
    path: str | os.PathLike[str],
    *,
    mode: str | None = None,
    time_limit_ms: int | None = None,
    mem_limit_mb: int = 0,
) -> Answer: ...
