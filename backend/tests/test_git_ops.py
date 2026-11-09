import os


def test_ambiente_de_teste_nao_herda_git_dir():
    # Backstop do fixture de sessão em conftest.py: se ele não rodou (ou GIT_* voltou depois),
    # `_repo` escreveria no `.git` real do processo, não no tmp_path isolado.
    assert not [k for k in os.environ if k.startswith("GIT_")]
