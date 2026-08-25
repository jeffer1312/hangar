"""Subagentes do Claude Code (`<sessao>/subagents/agent-<id>.jsonl`).

O caminho do Kimi ganhou teste primeiro (test_subagents_kimi.py) porque a leitura dele nasceu agora,
mas o furo do "transcript ilegivel some da lista" era IGUAL nos dois — foi corrigido nos dois e
precisa de trava nos dois, senao o do Claude volta na primeira mexida.
"""

import json

from app.subagents import get_subagent, list_subagents


def _linhas(agent: str) -> str:
    return "\n".join(json.dumps(x, ensure_ascii=False) for x in [
        {"type": "user", "timestamp": "2026-08-24T17:00:00Z",
         "message": {"content": f"Investigue o {agent}."}},
        {"type": "assistant", "timestamp": "2026-08-24T17:00:05Z", "message": {"content": [
            {"type": "tool_use", "name": "Bash", "input": {"command": "ls"}},
            {"type": "text", "text": f"Relatorio de {agent}."},
        ]}},
    ]) + "\n"


def _sessao(tmp_path, agentes: list[str]) -> str:
    """Monta <tmp>/s/subagents/agent-<id>.jsonl e devolve o caminho do jsonl da sessao."""
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("", encoding="utf-8")
    d = tmp_path / "s" / "subagents"
    d.mkdir(parents=True)
    for a in agentes:
        (d / f"agent-{a}.jsonl").write_text(_linhas(a), encoding="utf-8")
    return str(jsonl)


def test_transcript_ilegivel_aparece_marcado_em_vez_de_sumir(tmp_path):
    main = _sessao(tmp_path, ["aa01", "aa02"])
    alvo = tmp_path / "s" / "subagents" / "agent-aa02.jsonl"
    alvo.chmod(0o000)
    try:
        ags = {a["agentId"]: a for a in list_subagents(main)}
    finally:
        alvo.chmod(0o644)
    assert set(ags) == {"aa01", "aa02"}
    assert ags["aa02"]["ilegivel"] is True
    assert "ilegivel" not in ags["aa01"]


def test_detalhe_de_ilegivel_nao_e_404(tmp_path):
    # O arquivo EXISTE — devolver None juntava "nao deu pra ler" com "nao existe" no mesmo 404, e a
    # linha que a lista marcou como ilegivel abria dizendo que o agente nem existe.
    main = _sessao(tmp_path, ["aa01"])
    alvo = tmp_path / "s" / "subagents" / "agent-aa01.jsonl"
    alvo.chmod(0o000)
    try:
        d = get_subagent(main, "aa01", events=10)
    finally:
        alvo.chmod(0o644)
    assert d is not None and d["ilegivel"] is True
    # Sem `events` falsamente vazio: uma lista vazia aqui faria a tela dizer "nenhuma ferramenta
    # chamada" pra um transcript que ninguem conseguiu abrir.
    assert "events" not in d


def test_agente_que_nao_existe_continua_sendo_404(tmp_path):
    main = _sessao(tmp_path, ["aa01"])
    assert get_subagent(main, "naoexiste") is None


def test_pasta_subagents_sumida_devolve_lista_vazia(tmp_path):
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("", encoding="utf-8")
    assert list_subagents(str(jsonl)) == []
