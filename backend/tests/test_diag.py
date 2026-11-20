"""diag: o diário de uso que a pessoa baixa e manda pra quem mantém o app.

Tudo contra a pasta temporária dos testes — nenhum caso escreve nos logs reais da máquina.
"""
import json
from datetime import date, datetime

import pytest

from app import diag


@pytest.fixture(autouse=True)
def config_falso(tmp_path, monkeypatch):
    monkeypatch.setenv("CLAUDE_CONFIG_DIR", str(tmp_path))
    return tmp_path


def _linhas(quando: date | None = None) -> list[dict]:
    p = diag.caminho_do_dia(quando)
    return [json.loads(x) for x in p.read_text(encoding="utf-8").splitlines() if x.strip()]


def test_registrar_nunca_levanta(monkeypatch):
    # Um diário que derruba o pedido que ele deveria descrever é pior que diário nenhum.
    monkeypatch.setattr(diag, "_escrever", lambda _l: (_ for _ in ()).throw(OSError("disco cheio")))
    diag.registrar("qualquer")   # não levanta


def test_registrar_carimba_o_id_do_pedido_em_curso():
    # É o que liga a linha da tela ("POST /select devolveu 409") à do servidor ("o cursor não
    # convergiu"). Sem o id, amarrar as duas depende de comparar horário — que empata com duas
    # telas abertas.
    token = diag.req_atual.set("abc123-7")
    try:
        diag.registrar("opcao.nao_convergiu", "erro", sessao="pmw")
    finally:
        diag.req_atual.reset(token)
    l = _linhas()[0]
    assert l["req"] == "abc123-7" and l["origem"] == "servidor"


def test_sem_pedido_em_curso_nao_inventa_id():
    diag.registrar("qualquer")
    assert "req" not in _linhas()[0]


def test_evento_do_proprio_backend_continua_com_o_horario_da_chegada():
    diag.registrar("tmux.mudo", nivel="erro")
    assert datetime.fromisoformat(_linhas()[0]["ts"]).date() == date.today()
