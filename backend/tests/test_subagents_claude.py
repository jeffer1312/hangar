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


def _erro_de_api(agent: str) -> str:
    return json.dumps({"type": "assistant", "timestamp": "2026-08-24T17:00:09Z", "isApiErrorMessage": True,
                       "error": "server_error", "apiErrorStatus": 529,
                       "message": {"content": [{"type": "text", "text": f"API Error: 529 Overloaded ({agent})"}]}}) + "\n"


def test_falha_vem_do_erro_de_api_na_ultima_resposta(tmp_path):
    main = _sessao(tmp_path, ["ok01", "falha01", "voltou01", "depois01", "texto01"])
    d = tmp_path / "s" / "subagents"
    with (d / "agent-falha01.jsonl").open("a", encoding="utf-8") as f:
        f.write(_erro_de_api("falha01"))
    # Registro que não é resposta depois do erro não desfaz a falha.
    with (d / "agent-depois01.jsonl").open("a", encoding="utf-8") as f:
        f.write(_erro_de_api("depois01"))
        f.write(json.dumps({"type": "user", "timestamp": "2026-08-24T17:00:30Z", "message": {"content": "ok"}}) + "\n")
    # Só o booleano do Claude Code conta; texto "true" não é falha.
    with (d / "agent-texto01.jsonl").open("a", encoding="utf-8") as f:
        f.write(_erro_de_api("texto01").replace('"isApiErrorMessage": true', '"isApiErrorMessage": "true"'))
    # Erro no meio e uma resposta normal depois: o agente seguiu, não falhou.
    with (d / "agent-voltou01.jsonl").open("a", encoding="utf-8") as f:
        f.write(_erro_de_api("voltou01"))
        f.write(json.dumps({"type": "assistant", "timestamp": "2026-08-24T17:00:20Z",
                            "message": {"content": [{"type": "text", "text": "Retomei."}]}}) + "\n")
    ags = {a["agentId"]: a for a in list_subagents(main)}
    assert ags["ok01"]["failed"] is False
    assert ags["falha01"]["failed"] is True
    assert ags["voltou01"]["failed"] is False
    assert ags["depois01"]["failed"] is True
    assert ags["texto01"]["failed"] is False
    assert get_subagent(main, "falha01")["failed"] is True
    # O campo diz só a falha: "terminou" continua sendo do tool_result no pai.
    assert "finished" not in ags["falha01"]


def test_ilegivel_nao_afirma_falha(tmp_path):
    main = _sessao(tmp_path, ["aa01"])
    alvo = tmp_path / "s" / "subagents" / "agent-aa01.jsonl"
    alvo.chmod(0o000)
    try:
        ags = list_subagents(main)
    finally:
        alvo.chmod(0o644)
    assert "failed" not in ags[0]


def test_pasta_subagents_sumida_devolve_lista_vazia(tmp_path):
    jsonl = tmp_path / "s.jsonl"
    jsonl.write_text("", encoding="utf-8")
    assert list_subagents(str(jsonl)) == []
