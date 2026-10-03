#!/usr/bin/env python3
"""Clientes de controle não contam como terminal local no snapshot do painel."""
import runpy
import subprocess
from pathlib import Path
from unittest.mock import patch

script = runpy.run_path(str(Path(__file__).with_name("hangar-panel-data")))


def attached(rows, error=False):
    def run(argv, **kwargs):
        assert argv[:3] == ["tmux", "list-clients", "-F"]
        if error:
            raise subprocess.CalledProcessError(1, argv)
        fields = argv[-1].split("\t")
        return subprocess.CompletedProcess(argv, 0, "\n".join(
            "\t".join(row[field] for field in fields) for row in rows), "")
    with patch.object(subprocess, "run", run):
        return script["attached_locally"]()


observer = {"#{session_name}": "observada", "#{client_control_mode}": "1"}
human = {"#{session_name}": "nome com espaco", "#{client_control_mode}": "0"}
assert attached([observer]) == set(), "observador não conta como terminal aberto"
assert attached([observer, human, human]) == {"nome com espaco"}, "preserva humano e deduplica"
assert attached([{**human, "#{client_control_mode}": ""}]) == set(), "modo desconhecido não comprova presença"
assert attached([human], error=True) == set(), "consulta falha conserva retorno vazio"
print("ok: 4 casos")
