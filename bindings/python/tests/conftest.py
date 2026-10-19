import pathlib

import pytest


def pytest_addoption(parser):
    parser.addoption(
        "--tididi-cli",
        metavar="PATH",
        help="the tididi executable, to compare its counts with tididi_cnf's",
    )


@pytest.fixture
def tididi_cli(request):
    path = request.config.getoption("--tididi-cli")
    if path is None:
        pytest.skip("give --tididi-cli to compare with the command line")
    return pathlib.Path(path)
