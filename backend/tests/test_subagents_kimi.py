"""Subagentes do Kimi (tools `Agent` e `AgentSwarm`).

O layout e outro: cada subagente tem uma PASTA, <sessao>/agents/agent-N/wire.jsonl, ao lado do
agents/main/wire.jsonl da conversa. As linhas aqui copiam o shape do wire REAL (medido numa sessao
do usuario em 24/08/2026) — inventar um shape "que a doc sugere" ja deixou passar bug de parser
antes (ver o `tool.result` sem uuid no CLAUDE.md).
"""

import json

from app.subagents import get_subagent, list_subagents


def _wire(linhas: list[dict]) -> str:
    return "\n".join(json.dumps(x, ensure_ascii=False) for x in linhas) + "\n"


def _loop(agent: str, ev: dict, t: int) -> dict:
    return {"type": "context.append_loop_event", "agentId": agent, "event": ev, "time": t}


def _sessao(tmp_path, agentes: dict[str, str]) -> str:
    """Monta <tmp>/agents/{main,...}/wire.jsonl e devolve o caminho do wire do main."""
    base = tmp_path / "agents"
    (base / "main").mkdir(parents=True)
    (base / "main" / "wire.jsonl").write_text(_wire([{"type": "metadata"}]), encoding="utf-8")
    for nome, conteudo in agentes.items():
        (base / nome).mkdir()
        (base / nome / "wire.jsonl").write_text(conteudo, encoding="utf-8")
    return str(base / "main" / "wire.jsonl")


def _subagente(nome: str, *, terminou: bool = True) -> str:
    return _wire([
        {"type": "metadata", "protocol_version": "1.5", "created_at": 1787601577157},
        {"type": "profile.bind", "agentId": nome, "profileName": "explore",
         "modelAlias": "apikey/k3", "time": 1787601577200},
        {"type": "turn.prompt", "agentId": nome, "time": 1787601577300, "input": [
            {"type": "text",
             "text": "<git-context>\nBranch: develop\n</git-context>\n\nAssistir o vídeo "
                     f"{nome}.mp4 e relatar."},
        ]},
        _loop(nome, {"type": "tool.call", "name": "Bash",
                     "args": {"command": f"watch.py {nome}.mp4"}}, 1787601577400),
        _loop(nome, {"type": "content.part",
                     "part": {"type": "think", "think": "isto é raciocínio, não fala"}},
              1787601577450),
        _loop(nome, {"type": "content.part",
                     "part": {"type": "text", "text": f"Relatório de {nome}."}}, 1787601577500),
        *([{"type": "turn.ended", "agentId": nome, "reason": "completed",
            "time": 1787601577600}] if terminou else []),
    ])


def test_lista_os_subagentes_do_kimi(tmp_path):
    main = _sessao(tmp_path, {"agent-2": _subagente("agent-2"),
                              "agent-3": _subagente("agent-3", terminou=False)})
    ags = {a["agentId"]: a for a in list_subagents(main)}
    assert set(ags) == {"agent-2", "agent-3"}
    a = ags["agent-2"]
    assert a["agentType"] == "explore"
    assert a["toolCalls"] == 1
    assert a["recent"][-1] == {"name": "Bash", "target": "watch.py agent-2.mp4"}
    # `lastText` e a FALA do subagente. O `think` do mesmo evento nao entra: e raciocinio, e a linha
    # da lista mostra "o que ele esta fazendo agora" pra quem le de fora.
    assert a["lastText"] == "Relatório de agent-2."
    assert a["finished"] is True
    assert ags["agent-3"]["finished"] is False


def test_prompt_perde_o_git_context(tmp_path):
    # O bloco <git-context> e IGUAL em todos os subagentes da sessao (branch, sujos, commits), e o
    # front usa a primeira linha do prompt como titulo — sem tirar, os quatro subagentes de um
    # AgentSwarm apareceriam com o mesmo "Branch: develop".
    main = _sessao(tmp_path, {"agent-2": _subagente("agent-2")})
    a = list_subagents(main)[0]
    assert a["prompt"] == "Assistir o vídeo agent-2.mp4 e relatar."


def test_main_nunca_aparece_como_subagente(tmp_path):
    # agents/main/ e a CONVERSA. Listar ele junto poria o transcript inteiro dentro do painel de
    # subagentes, como se a sessao fosse filha de si mesma.
    main = _sessao(tmp_path, {"agent-0": _subagente("agent-0")})
    assert [a["agentId"] for a in list_subagents(main)] == ["agent-0"]


def test_detalhe_traz_os_eventos_no_formato_do_chat(tmp_path):
    main = _sessao(tmp_path, {"agent-2": _subagente("agent-2")})
    d = get_subagent(main, "agent-2", events=10)
    assert d is not None and d["agentId"] == "agent-2"
    # Os eventos passam pelo parser do KIMI, nao pelo do Claude: o wire nao e o jsonl do Claude e o
    # parser errado devolveria lista vazia — um subagente que trabalhou parecendo parado.
    assert any(e.get("kind") == "tool_use" for e in d["events"])


def test_agent_id_de_fora_da_pasta_nao_abre_nada(tmp_path):
    # `agent_id` vem da URL. Sem a trava, "../main" (ou um caminho absoluto) leria o wire da
    # conversa — ou o de outra sessao — pela rota de detalhe do subagente.
    main = _sessao(tmp_path, {"agent-2": _subagente("agent-2")})
    assert get_subagent(main, "../main") is None
    assert get_subagent(main, "main") is None
    assert get_subagent(main, "agent-9") is None


def test_agent_id_com_caminho_absoluto_nao_escapa(tmp_path):
    # `Path("<...>/agents") / "/outro/lugar"` devolve "/outro/lugar" — a base e DESCARTADA, e no
    # Windows o mesmo vale pra "D:x", que nao tem separador nenhum e ainda assim e drive-relativo.
    # Por isso a trava e o caminho RESOLVIDO, e nao uma lista de caracteres proibidos.
    fora = tmp_path / "outra-sessao" / "agents" / "agent-2"
    fora.mkdir(parents=True)
    (fora / "wire.jsonl").write_text(_subagente("agent-2"), encoding="utf-8")
    main = _sessao(tmp_path / "minha", {"agent-2": _subagente("agent-2")})
    assert get_subagent(main, str(fora)) is None
    assert get_subagent(main, "../../outra-sessao/agents/agent-2") is None
    # E o caminho legitimo continua abrindo — trava que recusa tudo tambem "passa" neste teste.
    assert get_subagent(main, "agent-2") is not None


def test_symlink_pra_fora_da_sessao_nao_abre(tmp_path):
    # O realpath resolve o link ANTES de comparar: sem isso, um agents/atalho -> outra sessao seria
    # filho direto da pasta certa no papel e leria o wire da sessao errada.
    fora = tmp_path / "outra-sessao" / "agents" / "agent-7"
    fora.mkdir(parents=True)
    (fora / "wire.jsonl").write_text(_subagente("agent-7"), encoding="utf-8")
    main = _sessao(tmp_path / "minha", {"agent-2": _subagente("agent-2")})
    (tmp_path / "minha" / "agents" / "atalho").symlink_to(fora, target_is_directory=True)
    assert get_subagent(main, "atalho") is None
