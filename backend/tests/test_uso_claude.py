import json
from datetime import datetime, timezone
from pathlib import Path

import pytest

from app import costs_cache as cc, costs_claude_transcript as ct, pricing, uso_areas, uso_report
from app.uso_claude import comando_bash, skill_do_caminho

T0 = "2026-09-10T12:00:00Z"


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(cc, "_CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    # O mapa real da máquina (~/.hangar/uso-areas.json) não pode mudar o resultado do teste.
    monkeypatch.setattr(uso_areas, "_arquivo", lambda: tmp_path / "uso-areas.json")
    uso_areas.recarregar()
    ct.invalidar_cache()
    yield
    ct.invalidar_cache()
    uso_areas.recarregar()


def _escrever(p: Path, linhas: list[dict]) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) for x in linhas) + "\n", encoding="utf-8")


def _usage(i=10, o=5, cw=0, cr=0):
    return {"input_tokens": i, "output_tokens": o, "cache_creation_input_tokens": cw,
            "cache_read_input_tokens": cr}


def _assistant(blocos, mid, usage=None, ts=T0, model="claude-opus-5"):
    return {"type": "assistant", "timestamp": ts, "cwd": "/repo", "requestId": "r" + mid,
            "message": {"id": mid, "model": model, "usage": usage or _usage(), "content": blocos}}


def _user(conteudo, prompt_id, ts=T0, **extra):
    return {"type": "user", "timestamp": ts, "cwd": "/repo", "promptId": prompt_id,
            "message": {"role": "user", "content": conteudo}, **extra}


def _tool_use(nome, entrada, tid):
    return {"type": "tool_use", "id": tid, "name": nome, "input": entrada}


def _tool_result(tid, texto):
    return {"type": "tool_result", "tool_use_id": tid, "content": texto}


def test_conta_tools_bash_mcp_e_mede_o_resultado(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("faz", "p1"),
        _assistant([_tool_use("Bash", {"command": "cd x && git status"}, "t1")], "m1"),
        _user([_tool_result("t1", "x" * 400)], "p1"),
        _assistant([_tool_use("Bash", {"command": "TOKEN=$(grep a b | cut -d= -f2); curl u"}, "t2")], "m2"),
        _user([_tool_result("t2", [{"type": "text", "text": "y" * 100}])], "p1"),
        _assistant([_tool_use("mcp__hangar__nav", {"url": "u"}, "t3")], "m3"),
        _user([_tool_result("t3", "z" * 40)], "p1"),
        _assistant([_tool_use("Read", {"file_path": "/f"}, "t4")], "m4"),
        _user([_tool_result("t4", "w" * 8)], "p1"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all")
    tools = {b.key: b for b in r.by_tool}
    assert tools["Bash"].chamadas == 2 and tools["Bash"].ctx_chars == 500
    assert tools["Read"].chamadas == 1 and tools["Read"].ctx_chars == 8
    assert tools["Read"].ctx_tokens_est == 2
    bash = {b.key: b for b in r.by_bash}
    assert bash["git"].chamadas == 1 and bash["git"].ctx_chars == 400
    assert bash["grep"].chamadas == 1 and bash["grep"].ctx_chars == 100
    mcp = {b.key: b for b in r.by_mcp}
    assert mcp["hangar"].chamadas == 1 and mcp["hangar"].ctx_chars == 40
    assert r.totals.chamadas == 4          # bash/mcp não duplicam a chamada do tool
    assert r.totals.ctx_chars == 548
    assert r.totals.sessions == 1


@pytest.mark.parametrize("cmd, esperado", [
    ("git status", "git"),
    ("cd /x && npm run build", "npm"),
    ("(cd sub && uv run pytest)", "uv"),
    ("CP_X=1 FOO=bar python3 -c 1", "python3"),
    ("TOKEN=$(grep a .env | cut -d= -f2); curl -s u", "grep"),
    ("sudo systemctl restart x", "systemctl"),
    ("timeout 300 npx vitest run", "npx"),
    ("/usr/bin/see img.png", "see"),
    ("for i in 1 2; do echo $i; done", "for"),
    ("", "?"),
])
def test_comando_bash(cmd, esperado):
    assert comando_bash(cmd) == esperado


def _texto_user(texto, prompt_id, **extra):
    return _user([{"type": "text", "text": texto}], prompt_id, **extra)


def _resposta(mid):
    return _assistant([{"type": "text", "text": "."}], mid)


def test_skill_ocupa_o_contexto_ate_o_fim_ou_a_compactacao(tmp_path):
    meta = "Base directory for this skill: /x\n" + "k" * 1000
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("usa a skill", "p1"),
        _resposta("m0"),                                              # antes da carga
        _assistant([_tool_use("Skill", {"skill": "acme:kubectl"}, "t1")], "m1"),
        _resposta("m1"),                                              # mesma resposta
        _user([_tool_result("t1", "Launching skill: acme:kubectl")], "p1"),
        _texto_user(meta, "p1", isMeta=True),
        _resposta("m2"),
        _user("outra coisa", "p2"),
        _resposta("m3"),                                              # prompt novo: continua lá
        # Comando embutido não expande texto: não é skill.
        _user("<command-message>model</command-message>\n<command-name>/model</command-name>", "p3"),
        _resposta("m4"),
        _user("<command-message>x</command-message>\n<command-name>/ecc:cost-report</command-name>", "p4"),
        _texto_user("e" * 300, "p4", isMeta=True),
        _resposta("m5"),
        {"type": "system", "subtype": "compact_boundary", "timestamp": T0, "cwd": "/repo"},
        _resposta("m6"),                                              # compactou: saiu tudo
        {"type": "attachment", "timestamp": T0, "cwd": "/repo",
         "attachment": {"type": "invoked_skills", "skills": [{"name": "acme:kubectl", "content": "k" * 500}]}},
        _resposta("m7"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all")
    skills = {b.key: b for b in r.by_skill}
    assert set(skills) == {"acme:kubectl", "ecc:cost-report"}
    k, c = skills["acme:kubectl"], skills["ecc:cost-report"]
    # m2..m5 com o texto inteiro; depois da compactação, só a reinjeção em m7.
    assert (k.chamadas, k.pedidas, k.plugin) == (1, 0, "acme")
    assert k.respostas == 5
    assert k.ocupados_tokens_est == int((len(meta) * 4 + 500) / 2.5)
    assert k.ctx_tokens_est == int((len(meta) + 500) / 2.5)
    assert (c.chamadas, c.pedidas, c.respostas) == (1, 1, 1)
    assert c.ocupados_tokens_est == int(300 / 2.5)
    assert {b.key: b.ocupados_tokens_est for b in r.by_plugin}["acme"] == k.ocupados_tokens_est


def test_skill_lida_como_arquivo_ou_colada_por_hook_conta_como_carga(tmp_path):
    sp = "/h/.claude/plugins/cache/mkt/superpowers/6.3.0/skills/writing-plans/SKILL.md"
    hook = ["<EXTREMELY_IMPORTANT>\nYou have superpowers.\n\n**Below is the full content of your "
            "'superpowers:using-superpowers' skill**\n" + "u" * 800]
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("x", "p1"),
        {"type": "attachment", "timestamp": T0, "cwd": "/repo",
         "attachment": {"type": "hook_additional_context", "hookName": "SessionStart", "content": hook}},
        {"type": "attachment", "timestamp": T0, "cwd": "/repo",
         "attachment": {"type": "hook_success", "hookName": "SessionStart:startup",
                        "content": "/last30days: Ready\nLast run: superpowers skill"}},
        _assistant([_tool_use("Read", {"file_path": sp}, "t1")], "m1"),
        _user([_tool_result("t1", "w" * 700)], "p1"),
        # Referência de skill ainda não carregada: é alguém mexendo na skill, fica no Bash.
        _assistant([_tool_use("Bash", {"command": "cat /h/p/skills/orquestrar/references/executor.md"}, "t2")], "m2"),
        _user([_tool_result("t2", "r" * 400)], "p1"),
        _assistant([_tool_use("Bash", {"command": "sed -n 1,200p /h/p/skills/orquestrar/SKILL.md"}, "t3")], "m3"),
        _user([_tool_result("t3", "o" * 900)], "p1"),
        _assistant([_tool_use("Read", {"file_path": "/h/p/skills/orquestrar/references/executor.md"}, "t4")], "m4"),
        _user([_tool_result("t4", "e" * 600)], "p1"),
        _resposta("m5"),
    ])
    uso = ct.varrer_uso(tmp_path)
    r = uso_report.montar(uso, [], "all")
    skills = {b.key: b for b in r.by_skill}
    wp = skills["superpowers:writing-plans"]
    assert (wp.chamadas, wp.plugin, wp.respostas) == (1, "superpowers", 4)
    assert wp.ocupados_tokens_est == int(700 * 4 / 2.5)
    using = skills["superpowers:using-superpowers"]
    assert (using.chamadas, using.respostas) == (1, 5)
    orq = skills["orquestrar"]
    # SKILL.md em m4 e m5; a referência (lida com a skill já carregada) só em m5.
    assert (orq.chamadas, orq.respostas) == (1, 2)
    assert orq.ocupados_tokens_est == int((900 * 2 + 600) / 2.5)
    tools = {b.key: b for b in r.by_tool}
    assert tools["Read"].ctx_chars == 0 and tools["Bash"].ctx_chars == 400
    ctx = {b.key: b for b in r.by_contexto}
    assert all(b.plugin == "" for b in ctx.values())                  # last30days não é superpowers
    assert not any("EXTREMELY" in k for k in ctx)                      # hook de skill não é contexto solto


@pytest.mark.parametrize("caminho,esperado", [
    ("/h/.claude/plugins/cache/mkt/superpowers/6.3.0/skills/brainstorming/SKILL.md", ("superpowers:brainstorming", True)),
    ("/h/.claude/plugins/marketplaces/acme-marketplace/skills/kubectl/SKILL.md", ("acme:kubectl", True)),
    ("/h/.claude/plugins/cache/mkt/mattpocock-skills/1.0.0/skills/eng/implement/SKILL.md", ("mattpocock-skills:implement", True)),
    ("/h/p/skills/orquestrar/references/executor.md", ("orquestrar", False)),
    ("/h/.agents/skills/svelte-code-writer/SKILL.md", ("svelte-code-writer", True)),
    ("/h/p/docs/skills.md", None),
])
def test_skill_do_caminho(caminho, esperado):
    assert skill_do_caminho(caminho) == esperado


def test_agente_liga_ao_transcript_filho_pelo_agentId(tmp_path, monkeypatch):
    monkeypatch.setattr(pricing, "rate_for", lambda m: pricing.Rate(
        provider="anthropic", input=1.0, output=10.0, cache_read=0.1, cache_write=1.25, origin="teste",
        cache_estimado=False))
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("pesquisa", "p1"),
        _assistant([_tool_use("Agent", {"subagent_type": "Explore", "prompt": "x"}, "t1")], "m1"),
        _user([_tool_result("t1", "lançado")], "p1", toolUseResult={"agentId": "abc123", "status": "async_launched"}),
        _assistant([_tool_use("Agent", {"prompt": "y"}, "t2")], "m2"),
        _user([_tool_result("t2", "lançado")], "p1", toolUseResult={"agentId": "def456"}),
    ])
    _escrever(tmp_path / "p" / "s1" / "subagents" / "agent-abc123.jsonl", [
        _assistant([{"type": "text", "text": "achei"}], "a1", _usage(i=1000, o=100)),
    ])
    uso = ct.varrer_uso(tmp_path)
    tokens = [__import__("app.costs_sources", fromlist=["UsageRow"]).UsageRow(
        ts=u.ts, source="claude", provider="anthropic", model=u.model, project=u.cwd,
        session_id=u.session_id, input=u.input, output=u.output, cache_write=u.cache_write,
        cache_read=u.cache_read, subagente=u.subagente) for u in ct.varrer(tmp_path)]
    r = uso_report.montar(uso, tokens, "all")
    ag = {b.key: b for b in r.by_agente}
    assert ag["Explore"].chamadas == 1
    assert (ag["Explore"].input, ag["Explore"].output) == (1000, 100)
    assert ag["Explore"].cost == pytest.approx(1000 / 1e6 + 100 / 1e6 * 10)
    assert ag["Explore"].cost_input == pytest.approx(1000 / 1e6)
    assert ag["Explore"].cost_output == pytest.approx(100 / 1e6 * 10)
    assert ag["Explore"].cost_cache_write == ag["Explore"].cost_cache_read == 0
    assert ag["general-purpose"].chamadas == 1 and ag["general-purpose"].cost == 0


def test_agente_pedido_ou_sozinho_pela_heuristica_do_prompt(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("dispara um subagente pra mapear isso", "p1"),
        _assistant([_tool_use("Agent", {"subagent_type": "Explore", "prompt": "x"}, "t1")], "m1"),
        _user([_tool_result("t1", "ok")], "p1", toolUseResult={"agentId": "a1"}),
        _user("conserta o bug do login", "p2"),
        _assistant([_tool_use("Agent", {"subagent_type": "Explore", "prompt": "y"}, "t2")], "m2"),
        _user([_tool_result("t2", "ok")], "p2", toolUseResult={"agentId": "a2"}),
        # Prompt em blocos (imagem + texto) também conta; o texto expandido (isMeta) não.
        _user([{"type": "text", "text": "roda os agents em paralelo"}], "p3"),
        _user([{"type": "text", "text": "sem agente nenhum aqui"}], "p3", isMeta=True),
        _assistant([_tool_use("Agent", {"subagent_type": "Explore", "prompt": "z"}, "t3")], "m3"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all")
    ag = {b.key: b for b in r.by_agente}
    assert (ag["Explore"].chamadas, ag["Explore"].pedidas) == (3, 2)


def test_filtro_plugin_inclui_agente_com_prefixo(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("revise", "p1"),
        _assistant([_tool_use("Agent", {"subagent_type": "ecc:typescript-reviewer", "prompt": "x"}, "t1")], "m1"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all", plugin="ecc")
    assert [(b.key, b.plugin, b.chamadas) for b in r.by_agente] == [("ecc:typescript-reviewer", "ecc", 1)]
    assert [(b.key, b.chamadas) for b in r.by_plugin] == [("ecc", 1)]


def test_contexto_mede_rendered_ou_content_e_ignora_stdout_de_hook(tmp_path):
    att = lambda a, **extra: {"type": "attachment", "timestamp": T0, "attachment": a, **extra}
    _escrever(tmp_path / "p" / "s1.jsonl", [
        att({"type": "instructions"}, rendered=[{"content": "i" * 1000}]),
        att({"type": "hook_success", "hookName": "SessionStart:startup",
             "content": "PONYTAIL MODE ACTIVE — level: full\nblá", "stdout": "s" * 5000},
            rendered=[{"content": "P" * 40}]),
        # Hook que só tem stdout: nada entrou no contexto.
        att({"type": "hook_success", "hookName": "PostToolUse:Edit", "content": "", "stdout": "s" * 70000}),
        # CLI antiga, sem `rendered`: mede o content.
        att({"type": "hook_additional_context", "hookName": "UserPromptSubmit",
             "content": ["[skill-suggester] Prompt casa com skills\n- x"]}),
        att({"type": "total_tokens_reminder", "text": "t" * 90}),
        _user("oi", "p1"),
        _assistant([{"type": "text", "text": "oi"}], "m1"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all")
    ctx = {b.key: b for b in r.by_contexto}
    assert ctx["instructions"].ctx_chars == 1000
    pony = ctx["hook_success:SessionStart:startup · PONYTAIL MODE ACTIVE — level: full"]
    assert pony.ctx_chars == 40 and pony.plugin == "ponytail"
    # Hook sem nada no contexto não é ocorrência — e contexto nunca soma em "chamadas".
    assert "hook_success:PostToolUse:Edit" not in ctx
    assert r.totals.chamadas == 0
    sug = ctx["hook_additional_context:UserPromptSubmit · [skill-suggester] Prompt casa com skills"]
    assert sug.ctx_chars == len("[skill-suggester] Prompt casa com skills\n- x") and sug.plugin == "skill-suggester"
    assert ctx["total_tokens_reminder"].ctx_chars == 90
    assert {b.key for b in r.by_plugin} == {"skill-suggester", "ponytail"}


def test_periodo_corta_pelo_dia_local_e_o_eco_volta(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _assistant([_tool_use("Read", {"file_path": "/a"}, "t1")], "m1", ts="2026-09-01T12:00:00Z"),
        _assistant([_tool_use("Read", {"file_path": "/b"}, "t2")], "m2", ts="2026-09-10T12:00:00Z"),
    ])
    agora = datetime(2026, 9, 10, 15, tzinfo=timezone.utc)
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "7d", now=agora)
    assert r.by_tool[0].chamadas == 1
    assert r.applied.period == "7d"
    assert uso_report.montar(ct.varrer_uso(tmp_path), [], "all", now=agora).by_tool[0].chamadas == 2


def test_filtro_por_conta_corta_tudo_menos_a_lista_de_contas(tmp_path, monkeypatch):
    from dataclasses import replace
    from app import costs_sources as cs
    monkeypatch.setattr(pricing, "rate_for", lambda m: pricing.Rate(
        provider="anthropic", input=1.0, output=10.0, cache_read=0.1, cache_write=1.25, origin="teste",
        cache_estimado=False))
    monkeypatch.setitem(cs._ROTULOS, "anthropic:a", "a@x.com")
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("x", "p1"),
        _assistant([_tool_use("Skill", {"skill": "orquestrar"}, "t1")], "m1"),
        _user([_tool_result("t1", "k")], "p1"),
        _assistant([{"type": "text", "text": "ok"}], "m2", _usage(i=100, o=0)),
        _assistant([_tool_use("Agent", {"subagent_type": "Explore", "prompt": "x"}, "t2")], "m3"),
        _user([_tool_result("t2", "l")], "p1", toolUseResult={"agentId": "ag1"}),
    ])
    _escrever(tmp_path / "p" / "s1" / "subagents" / "agent-ag1.jsonl", [
        _assistant([{"type": "text", "text": "achei"}], "a1", _usage(i=1000, o=0)),
    ])
    linhas = ct.varrer_uso(tmp_path)
    uso = [replace(l, conta="anthropic:a") for l in linhas] + [replace(l, conta="anthropic:b") for l in linhas]
    tokens = []
    for conta in ("anthropic:a", "anthropic:b"):
        tokens += [cs.UsageRow(ts=u.ts, source="claude", provider=conta, model=u.model, project=u.cwd,
                               session_id=u.session_id, input=u.input, output=u.output,
                               cache_write=u.cache_write, cache_read=u.cache_read,
                               subagente=u.subagente, account_id=conta) for u in ct.varrer(tmp_path)]
    tudo = uso_report.montar(uso, tokens, "all")
    assert {b.key: b.label for b in tudo.by_conta} == {"anthropic:a": "a@x.com", "anthropic:b": None}
    assert tudo.by_skill[0].chamadas == 2 and tudo.by_agente[0].chamadas == 2
    assert tudo.conta == []
    so_a = uso_report.montar(uso, tokens, "all", conta="anthropic:a")
    assert so_a.conta == ["anthropic:a"]
    # Várias contas de uma vez: soma das duas, sem duplicar o seletor.
    duas = uso_report.montar(uso, tokens, "all", conta=["anthropic:a", "anthropic:b"])
    assert duas.by_skill[0].chamadas == 2 and len(duas.by_conta) == 2
    assert so_a.by_skill[0].chamadas == 1                                        # uma conta
    assert so_a.by_agente[0].chamadas == 1 and so_a.by_agente[0].input == 1000   # filho da conta certa
    assert [b.key for b in so_a.by_conta] == [b.key for b in tudo.by_conta]        # seletor inteiro


def _png_b64(largura: int, altura: int) -> str:
    import base64, struct
    cabecalho = b"\x89PNG\r\n\x1a\n" + struct.pack(">I", 13) + b"IHDR" + struct.pack(">II", largura, altura)
    return base64.b64encode(cabecalho + b"\x08\x06\x00\x00\x00" + b"x" * 40).decode()


def test_imagens_enviadas_e_lidas_com_tokens_pelos_pixels(tmp_path):
    img = lambda w, h: {"type": "image", "source": {"type": "base64", "media_type": "image/png", "data": _png_b64(w, h)}}
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user([{"type": "text", "text": "olha"}, img(1500, 750)], "p1"),
        _assistant([_tool_use("Read", {"file_path": "/x.png"}, "t1")], "m1"),
        _user([{"type": "tool_result", "tool_use_id": "t1", "content": [img(750, 750)]}], "p1"),
        _user([{"type": "text", "text": "e essa"}, {"type": "image", "source": {"type": "base64", "media_type": "image/jpeg", "data": "abcd"}}], "p2"),
    ])
    r = uso_report.montar(ct.varrer_uso(tmp_path), [], "all")
    im = {b.key: b for b in r.by_imagem}
    assert im["enviada"].chamadas == 2
    assert im["enviada"].ctx_tokens_est == 1500 * 750 // 750 + 0     # jpeg conta, sem estimativa
    assert im["lida:Read"].chamadas == 1 and im["lida:Read"].ctx_tokens_est == 750
    assert r.totals.chamadas == 4                                     # Read + 3 imagens


def test_filtros_projeto_modelo_plugin_e_serie_diaria_com_foco(tmp_path):
    def linha(dia, cwd, model, skill):
        return [
            {**_user("x", f"p-{dia}-{skill}", ts=f"{dia}T12:00:00Z"), "cwd": cwd},
            {**_assistant([_tool_use("Skill", {"skill": skill}, f"t-{dia}-{skill}")], f"m-{dia}-{skill}",
                          ts=f"{dia}T12:00:01Z", model=model), "cwd": cwd},
        ]
    _escrever(tmp_path / "p" / "s1.jsonl",
              linha("2026-09-01", "/a", "claude-opus-5", "ecc:x")
              + linha("2026-09-02", "/b", "claude-sonnet-5", "acme:y")
              + linha("2026-09-02", "/a", "claude-opus-5", "acme:y"))
    uso = ct.varrer_uso(tmp_path)
    tudo = uso_report.montar(uso, [], "all")
    assert [b.key for b in tudo.by_projeto] == ["/a", "/b"] or {b.key for b in tudo.by_projeto} == {"/a", "/b"}
    assert {b.key for b in tudo.by_modelo} == {"claude-opus-5", "claude-sonnet-5"}
    assert [(b.key, b.chamadas) for b in tudo.by_day] == [("2026-09-01", 1), ("2026-09-02", 2)]
    so_a = uso_report.montar(uso, [], "all", projeto="/a")
    assert so_a.totals.chamadas == 2 and {b.key for b in so_a.by_projeto} == {"/a", "/b"}
    so_sonnet = uso_report.montar(uso, [], "all", modelo="claude-sonnet-5")
    assert [b.key for b in so_sonnet.by_skill] == ["acme:y"] and so_sonnet.totals.chamadas == 1
    so_acme = uso_report.montar(uso, [], "all", plugin="acme")
    assert {b.key for b in so_acme.by_skill} == {"acme:y"} and so_acme.plugin == ["acme"]
    foco = uso_report.montar(uso, [], "all", foco="ecc:x")
    assert [(b.key, b.chamadas) for b in foco.by_day] == [("2026-09-01", 1)]
    assert len(foco.by_skill) == 2                                   # foco só recorta a série


def test_uso_sobrevive_ao_cache_em_disco(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _assistant([_tool_use("Read", {"file_path": "/a"}, "t1")], "m1"),
    ])
    antes = ct.varrer_uso(tmp_path)
    ct.invalidar_cache()
    assert ct.varrer_uso(tmp_path) == antes
    assert antes[0].session_id == "p/s1"


def test_skill_sem_prefixo_ganha_o_grupo_da_pasta_de_onde_veio(tmp_path, monkeypatch):
    home, repo = tmp_path / "home", tmp_path / "repo"
    monkeypatch.setattr(uso_report, "_REPO", repo)

    def skill(pasta: Path) -> Path:
        pasta.mkdir(parents=True)
        (pasta / "SKILL.md").write_text("x", encoding="utf-8")
        return pasta
    skill(repo / "skills" / "orquestrar")
    (home / ".claude" / "skills").mkdir(parents=True)
    (home / ".claude" / "skills" / "orquestrar").symlink_to(repo / "skills" / "orquestrar")
    skill(home / ".claude" / "skills" / "minha")
    (home / ".claude" / "skills" / "avulsa").symlink_to(skill(home / ".agents" / "skills" / "avulsa"))
    skill(home / ".codex" / "plugins" / "cache" / "mkt" / "superpowers" / "6.3.0" / "skills" / "brainstorming")
    origens = uso_report.origens_de_skill(home)
    assert {k: origens.get(k) for k in ("orquestrar", "minha", "avulsa", "brainstorming")} == {
        "orquestrar": "@repo", "minha": "@pessoal", "avulsa": "@avulsa", "brainstorming": "superpowers"}

    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("x", "p1"),
        _assistant([_tool_use("Skill", {"skill": n}, f"t{i}") for i, n in
                    enumerate(["orquestrar", "brainstorming", "sumida", "acme:k"])], "m1"),
    ])
    uso = ct.varrer_uso(tmp_path / "p")
    r = uso_report.montar(uso, [], "all", origens=origens)
    assert {b.key: b.plugin for b in r.by_skill} == {
        "orquestrar": "@repo", "brainstorming": "superpowers", "sumida": "@embutida", "acme:k": "acme"}
    # O filtro de plugin enxerga o grupo resolvido.
    r = uso_report.montar(uso, [], "all", plugin="superpowers", origens=origens)
    assert [b.key for b in r.by_skill] == ["brainstorming"]

    # Pasta ilegível perde só as skills dela.
    trancada = home / ".agents" / "skills"
    trancada.chmod(0)
    try:
        origens = uso_report.origens_de_skill(home)
    finally:
        trancada.chmod(0o755)
    assert (origens.get("orquestrar"), origens.get("brainstorming")) == ("@repo", "superpowers")


def test_skill_origins_rescan_only_when_a_skill_dir_changes(tmp_path, monkeypatch):
    home = tmp_path / "home"
    monkeypatch.setattr(uso_report, "_REPO", tmp_path / "repo")
    monkeypatch.setattr(uso_report, "_origens_cache", (float("-inf"), {}))
    monkeypatch.setattr(uso_report, "_origins_watch", ([], ()))
    skills = home / ".claude" / "plugins" / "cache" / "mkt" / "p" / "1.0" / "skills"
    (skills / "a").mkdir(parents=True)
    (skills / "a" / "SKILL.md").write_text("x", encoding="utf-8")
    scans = []
    original = uso_report.origens_de_skill
    monkeypatch.setattr(uso_report, "origens_de_skill", lambda h=None: scans.append(h) or original(h))

    uso_report._atualizar_origens(home, force=True)
    stamp, found = uso_report._origens_cache
    assert set(found) == {"a"}
    # Nada mudou: nem varre, nem troca o instante que chaveia o relatório pronto.
    uso_report._atualizar_origens(home)
    assert len(scans) == 1 and uso_report._origens_cache[0] == stamp

    (skills / "b").mkdir()
    (skills / "b" / "SKILL.md").write_text("x", encoding="utf-8")
    uso_report._atualizar_origens(home)
    assert len(scans) == 2 and set(uso_report._origens_cache[1]) == {"a", "b"}


def test_montar_accepts_index_tuples_like_uso_linha(tmp_path):
    _escrever(tmp_path / "p" / "s1.jsonl", [
        _user("x", "p1"),
        _assistant([_tool_use("Skill", {"skill": "orquestrar"}, "t1"),
                    _tool_use("Bash", {"command": "ls"}, "t2")], "m1", _usage(i=10)),
    ])
    uso = ct.varrer_uso(tmp_path / "p")
    rows = iter([tuple(getattr(l, c) for c in uso_report.FIELDS) for l in uso])
    assert uso_report.montar(rows, [], "all") == uso_report.montar(uso, [], "all")
