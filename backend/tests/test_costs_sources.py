import json
from pathlib import Path

import pytest

from app import costs_claude_transcript as ct
from app import codex_contas
from app import codex_contas
from app import costs_sources as cs
from app import pricing


@pytest.fixture(autouse=True)
def _pricing_isolado(tmp_path, monkeypatch):
    """Os leitores resolvem PROVEDOR via pricing, então sem isto o teste lê o cache real em
    ~/.claude/.hangar-pricing/ e passa ou falha conforme o estado da máquina — não do
    código. Mesma fixture do test_pricing.py.

    `ct._CACHE_DIR` também precisa de isolamento agora que `linhas_claude` delega pro transcript
    (Task 2): sem isto, cada teste grava um arquivo de cache real em
    ~/.claude/.hangar-custos/ — lixo que se acumula a cada rodada da suíte."""
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    monkeypatch.setattr(ct, "_CACHE_DIR", tmp_path / "custos")
    monkeypatch.setattr(Path, "home", classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    # getattr: `cs.invalidar_cache` só nasce na Task 6; até lá o no-op mantém a fixture válida
    # desde a Task 3, sem precisar reescrevê-la depois.
    limpar = lambda: getattr(cs, "invalidar_cache", lambda: None)()  # noqa: E731
    pricing.invalidar_cache(); ct.invalidar_cache(); limpar()
    yield
    pricing.invalidar_cache(); ct.invalidar_cache(); limpar()


def _escrever(p, linhas):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) if isinstance(x, (dict, list)) else x for x in linhas))


def _transcript_claude(cfg, sid, model, cwd, i=1, o=0, cw=0, cr=0, ts="2026-08-01T10:00:00Z"):
    """Escreve um transcript de conversa (não-subagente) mínimo, no formato que
    costs_claude_transcript.ler_transcript espera."""
    p = cfg / "projects" / "p" / f"{sid}.jsonl"
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text(json.dumps({
        "type": "assistant", "timestamp": ts, "sessionId": sid, "cwd": cwd,
        "message": {"model": model, "usage": {
            "input_tokens": i, "output_tokens": o,
            "cache_creation_input_tokens": cw, "cache_read_input_tokens": cr}}}),
        encoding="utf-8")
    return p


def test_ler_jsonl_pula_linha_invalida_e_nao_dict(tmp_path):
    p = tmp_path / "x.jsonl"
    _escrever(p, [{"a": 1}, "{quebrado", "null", "[1,2]", {"b": 2}])
    assert list(cs._ler_jsonl(p)) == [{"a": 1}, {"b": 2}]


# `cwd_do_transcript` foi removida junto com a `linhas_claude` antiga: era usada só por ela
# (resolvia o cwd a partir de `transcript_path` do costs.jsonl). O novo `linhas_claude` lê o
# `cwd` que o próprio `costs_claude_transcript.UsoSessao` já entrega — não precisa mais existir.

# As mecânicas de leitura do transcript em si (soma por turno, IGNORADOS pulado inteiro, ts do
# PRIMEIRO turno, tz-aware, linha inválida) já são cobertas em test_costs_claude_transcript.py
# (Task 1) — o que fica aqui é só o que é específico do WRAPPER linhas_claude: resolução de
# provedor, fallback de projeto e o arquivo/raiz ausente.


def test_sessao_de_motor_ganha_provedor_do_modelo(tmp_path):
    # CP_ENGINE só existe em /proc de sessão VIVA. Numa linha de ontem, quem entrega o
    # provedor é o modelo: 'k3' só existe na Moonshot.
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "k3", "/r", i=5, o=1)
    assert cs.linhas_claude(cfg, "conta-x")[0].provider == "moonshotai"


def test_transcript_sem_cwd_vira_projeto_desconhecido(tmp_path):
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "claude-opus-5", "", i=1, o=1)
    assert cs.linhas_claude(cfg, "c")[0].project == cs.PROJETO_DESCONHECIDO


def test_codex_usa_o_ultimo_token_count_e_desconta_o_cache(tmp_path, monkeypatch):
    # input_tokens do Codex INCLUI o cacheado. E reasoning_output é SUBCONJUNTO do output:
    # medido, 16242 + 18 = 16260. Somar dobra o output.
    raiz = tmp_path / "sessions" / "2026" / "07" / "30"
    _escrever(raiz / "rollout-2026-07-30T15-48-56-abc.jsonl", [
        {"timestamp": "2026-07-30T18:48:59Z", "type": "session_meta",
         "payload": {"cwd": "/repo/dois", "model_provider": "openai", "session_id": "abc"}},
        {"type": "turn_context", "payload": {"model": "gpt-5.6-sol"}},
        {"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {
            "input_tokens": 100, "cached_input_tokens": 40, "output_tokens": 5,
            "reasoning_output_tokens": 3}}}},
        {"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {
            "input_tokens": 300, "cached_input_tokens": 200, "output_tokens": 9,
            "reasoning_output_tokens": 4}}}},
    ])
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sessions")
    r = cs.linhas_codex()[0]
    assert r.input == 100      # 300 - 200
    assert r.cache_read == 200
    assert r.output == 9       # reasoning NÃO somado
    assert r.cache_write == 0  # cache da OpenAI é automático, não cobrado
    assert r.source == "codex"
    assert r.provider == "openai"
    assert r.project == "/repo/dois"
    assert r.model == "gpt-5.6-sol"


def test_codex_sem_diretorio_devolve_lista_vazia(tmp_path, monkeypatch):
    # Quem não usa Codex não pode ver "Codex: US$ 0,00" — isso lê como "usei e não gastou".
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "nao-existe")
    assert cs.linhas_codex() == []


def test_codex_respostas_reais_separam_dia_modelo_e_preservam_identidade_do_filho(tmp_path):
    def uso(tipo, ts, turno, entrada, cache, saida, resposta="r1", thread="filho"):
        u = {"input_tokens": entrada, "cached_input_tokens": cache,
             "output_tokens": saida, "reasoning_output_tokens": saida - 1}
        p = ({"thread_id": thread, "turn_id": turno, "response_id": resposta, "usage": u}
             if tipo == "token_usage_record" else
             {"type": "token_count", "info": {"total_token_usage": u}})
        return {"type": tipo, "timestamp": ts, "payload": p}

    primeiro = "2026-09-09T23:00:00Z"
    segundo = "2026-09-10T12:00:00Z"
    p = tmp_path / "sessions" / "rollout-filho.jsonl"
    registro = uso("token_usage_record", primeiro, "t1", 100, 80, 10)
    _escrever(p, [
        {"type": "session_meta", "timestamp": primeiro, "payload": {
            "id": "filho", "session_id": "pai", "cwd": "/filho", "model_provider": "openai",
            "source": {"subagent": {"thread_spawn": {"parent_thread_id": "pai"}}}}},
        {"type": "session_meta", "timestamp": primeiro,
         "payload": {"id": "pai", "session_id": "pai", "cwd": "/pai"}},
        {"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-5.6-luna"}},
        uso("token_usage_record", primeiro, "pai-t1", 999, 0, 99, "rp", "pai"),
        registro, registro,
        uso("event_msg", primeiro, "t1", 100, 80, 10),
        {"type": "turn_context", "payload": {"turn_id": "t2", "model": "gpt-6-astra"}},
        uso("token_usage_record", segundo, "t2", 50, 20, 5, "r2"),
        uso("event_msg", segundo, "t2", 50, 20, 5),
    ])
    rows = cs.linhas_codex(tmp_path)
    assert [(r.ts.day, r.model, r.input, r.cache_read, r.output) for r in rows] == [
        (9, "gpt-5.6-luna", 20, 80, 10), (10, "gpt-6-astra", 30, 20, 5)]
    assert all(r.session_id == "filho" and r.project == "/filho" and r.subagente for r in rows)


def test_codex_legado_nao_perde_uso_ao_retomar_nem_soma_repeticoes(tmp_path):
    linhas = _rollout_codex("/r", "s", {"input_tokens": 100, "cached_input_tokens": 80,
                                        "output_tokens": 10})
    linhas.extend([linhas[-1],
        {"type": "turn_context", "payload": {"turn_id": "novo", "model": "gpt-6-astra"}},
        {"type": "event_msg", "timestamp": "2026-09-10T12:00:00Z", "payload": {
            "type": "token_count", "info": {"total_token_usage": {
                "input_tokens": 50, "cached_input_tokens": 20, "output_tokens": 5}}}},
    ])
    _escrever(tmp_path / "sessions" / "rollout-s.jsonl", linhas)
    rows = cs.linhas_codex(tmp_path)
    assert sum(r.input for r in rows) == 50
    assert sum(r.cache_read for r in rows) == 100
    assert sum(r.output for r in rows) == 15
    assert rows[-1].ts.day == 10


def test_codex_cache_write_e_subconjunto_da_entrada(tmp_path):
    linhas = _rollout_codex("/r", "s", {
        "input_tokens": 100, "cached_input_tokens": 60, "cache_write_input_tokens": 30,
        "output_tokens": 10, "reasoning_output_tokens": 9})
    _escrever(tmp_path / "sessions" / "rollout-s.jsonl", linhas)
    (r,) = cs.linhas_codex(tmp_path)
    assert (r.input, r.cache_read, r.cache_write, r.output) == (10, 60, 30, 10)


def test_codex_contexto_longo_e_por_resposta_incluindo_cache(tmp_path):
    linhas = _rollout_codex("/r", "s", {"input_tokens": 272_000, "output_tokens": 1})[:2]
    for i, entrada in enumerate((272_000, 272_001)):
        linhas.append({"type": "token_usage_record", "timestamp": "2026-09-10T12:00:00Z",
                       "payload": {"thread_id": "s", "response_id": f"r{i}", "usage": {
                           "input_tokens": entrada, "cached_input_tokens": 270_000,
                           "output_tokens": 10}}})
    _escrever(tmp_path / "sessions" / "rollout-s.jsonl", linhas)
    rows = cs.linhas_codex(tmp_path)
    assert len(rows) == 2
    assert {(r.input, r.codex_long_context) for r in rows} == {(2000, False), (2001, True)}


def test_codex_rollout_sem_token_count_e_pulado(tmp_path, monkeypatch):
    raiz = tmp_path / "sessions" / "2026" / "08" / "01"
    _escrever(raiz / "rollout-x.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
         "payload": {"cwd": "/r", "model_provider": "openai"}},
    ])
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sessions")
    assert cs.linhas_codex() == []


def _rollout_codex(cwd, sid, tokens):
    return [
        {"timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
         "payload": {"cwd": cwd, "model_provider": "openai", "session_id": sid}},
        {"type": "turn_context", "payload": {"model": "gpt-5.6-sol"}},
        {"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": tokens}}},
    ]


def test_codex_le_sessions_e_archived_sessions_sem_duplicar(tmp_path, monkeypatch):
    # Arquivar MOVE (não copia): sessions/ e archived_sessions/ são diretórios IRMÃOS, e
    # sem varrer os dois o gasto da thread arquivada some do painel.
    _escrever(tmp_path / "sessions" / "rollout-viva.jsonl",
              _rollout_codex("/repo/viva", "viva", {"input_tokens": 10, "output_tokens": 1}))
    _escrever(tmp_path / "archived_sessions" / "rollout-arquivada.jsonl",
              _rollout_codex("/repo/arquivada", "arquivada", {"input_tokens": 20, "output_tokens": 2}))
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sessions")
    sids = {r.session_id for r in cs.linhas_codex()}
    assert sids == {"viva", "arquivada"}


def test_codex_sem_archived_sessions_nao_quebra(tmp_path, monkeypatch):
    _escrever(tmp_path / "sessions" / "rollout-viva.jsonl",
              _rollout_codex("/repo/viva", "viva", {"input_tokens": 10, "output_tokens": 1}))
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sessions")
    assert [r.session_id for r in cs.linhas_codex()] == ["viva"]


def test_pi_soma_as_mensagens_e_inclui_o_subagente(tmp_path, monkeypatch):
    # O Pi acumula POR MENSAGEM (não é snapshot cumulativo): tem que somar.
    # E o subagente mora em <sessao>/<taskId>/run-N/session.jsonl. Medido: na janela em que o
    # filho roda, o pai tem ZERO eventos de uso — não há duplicação, tem que somar os dois.
    raiz = tmp_path / "sessions" / "--repo-tres--"
    sess = raiz / "2026-07-30T20-29-24-651Z_18e48e08"
    _escrever(sess.with_suffix(".jsonl"), [
        {"type": "session", "timestamp": "2026-07-30T20:29:24Z", "cwd": "/repo/tres"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3-256k"},
        {"type": "message", "message": {"usage": {"input": 10, "output": 2,
                                                  "cacheRead": 100, "cacheWrite": 5}}},
        {"type": "message", "message": {"usage": {"input": 20, "output": 3,
                                                  "cacheRead": 200, "cacheWrite": 0}}},
    ])
    _escrever(sess / "44bad0fb" / "run-0" / "session.jsonl", [
        {"type": "session", "timestamp": "2026-07-30T20:52:12Z", "cwd": "/repo/tres"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3"},
        {"type": "message", "message": {"usage": {"input": 7, "output": 1,
                                                  "cacheRead": 50, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    linhas = sorted(cs.linhas_pi(), key=lambda r: r.input)
    assert len(linhas) == 2, "o subagente é uma linha própria, não pode sumir"
    filho, pai = linhas
    assert (filho.input, filho.output, filho.cache_read) == (7, 1, 50)
    assert filho.session_id != pai.session_id, "todo subagente se chama session.jsonl"
    assert (pai.input, pai.output, pai.cache_read, pai.cache_write) == (30, 5, 300, 5)
    assert pai.provider == "moonshotai", "'kimi-coding' é apelido; a chave é o provedor canônico"
    assert pai.project == "/repo/tres"
    assert pai.source == "pi"


def test_pi_usa_o_ultimo_provedor_declarado(tmp_path, monkeypatch):
    raiz = tmp_path / "sessions" / "--r--"
    _escrever(raiz / "s.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "openrouter", "modelId": "openrouter/free"},
        {"type": "model_change", "provider": "openai-codex", "modelId": "gpt-5.6-sol"},
        {"type": "message", "message": {"usage": {"input": 1, "output": 1,
                                                  "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    r = cs.linhas_pi()[0]
    assert r.provider == "openai", "'openai-codex' do Pi é a MESMA assinatura do 'openai' do Codex"
    assert r.model == "gpt-5.6-sol"


def test_as_tres_fontes_convergem_no_mesmo_provedor(tmp_path, monkeypatch):
    """Sem normalizar, a mesma assinatura fica partida em várias linhas do painel: OpenAI
    aparecia como 'openai' (Claude+Codex) e 'openai-codex' (Pi), e a Moonshot como
    'kimi-coding', 'clinepass' e 'moonshotai' — sempre o mesmo gpt-5.6-sol / k3. A pergunta
    'quanto minha assinatura da OpenAI rendeu' não tinha resposta na tela."""
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "gpt-5.6-sol", "/r", i=1, o=1)
    codex = tmp_path / "codex"
    _escrever(codex / "rollout-a.jsonl",
              _rollout_codex("/r", "a", {"input_tokens": 1, "output_tokens": 1}))
    pi = tmp_path / "pi"
    _escrever(pi / "s.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "openai-codex", "modelId": "gpt-5.6-sol"},
        {"type": "message", "message": {"usage": {"input": 1, "output": 1}}},
    ])
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: codex)
    monkeypatch.setattr(cs, "raiz_pi", lambda: pi)
    provs = {cs.linhas_claude(cfg, "conta-x")[0].provider,
             cs.linhas_codex()[0].provider, cs.linhas_pi()[0].provider}
    assert provs == {"openai"}, "as três fontes têm que cair na MESMA linha do painel"


def test_conta_anthropic_passa_intacta_pela_normalizacao(tmp_path):
    """'anthropic:<uuid>' é identidade de CONTA, não apelido de provedor: normalizar não pode
    encostar nela (duas contas Anthropic viram uma só se a chave for achatada)."""
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "claude-opus-5", "/r", i=1, o=1)
    assert cs.linhas_claude(cfg, "anthropic:758a9521-e2ef")[0].provider == "anthropic:758a9521-e2ef"


def test_pi_ignora_sessao_sem_uso(tmp_path, monkeypatch):
    raiz = tmp_path / "sessions" / "--r--"
    _escrever(raiz / "vazia.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    assert cs.linhas_pi() == []


def test_codex_le_modelo_so_do_turn_context(tmp_path, monkeypatch):
    # `model` só é confiável vindo de turn_context; um payload.model de outro tipo de evento
    # não pode vazar pro campo modelo da linha.
    raiz = tmp_path / "sessions"
    _escrever(raiz / "rollout-x.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
         "payload": {"cwd": "/r", "model_provider": "openai", "session_id": "x"}},
        {"type": "outro_evento", "payload": {"model": "modelo-errado"}},
        {"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {
            "input_tokens": 10, "output_tokens": 1}}}},
    ])
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: raiz)
    assert cs.linhas_codex()[0].model == "?"


def test_coletar_junta_as_tres_fontes_e_dedup_e_por_fonte(tmp_path, monkeypatch):
    # Chave de dedup é (fonte, session_id): um uuid repetido entre fontes não pode se comer.
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "mesmo-id", "claude-opus-5", "/r", i=1, o=1)
    _escrever(tmp_path / "sessions" / "--r--" / "mesmo-id.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3"},
        {"type": "message", "message": {"usage": {"input": 2, "output": 2,
                                                  "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "sem-omp")
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sem-codex")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "sem-kimi")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [(str(cfg), "conta-x")])
    cs.invalidar_cache()
    linhas = cs.coletar()
    assert {r.source for r in linhas} == {"claude", "pi"}
    assert len(linhas) == 2


def test_cache_relê_quando_o_arquivo_muda(tmp_path, monkeypatch):
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "claude-opus-5", "/r", i=1, o=0)
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "x")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "sem-omp")
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "y")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "z")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [(str(cfg), "c")])
    cs.invalidar_cache()
    assert len(cs.coletar()) == 1
    _transcript_claude(cfg, "s2", "claude-opus-5", "/r", i=1, o=0)
    assert len(cs.coletar()) == 2


def test_cache_evita_reparse_quando_nada_muda(tmp_path, monkeypatch):
    """O Pi continua com entrada própria em `_cache` (parser roda 1x sem tocar arquivo). O
    Claude NÃO tem mais entrada nesse cache (Step 4 desta tarefa): `linhas_claude` é chamado a
    cada `coletar()` — quem evita reler o mesmo transcript é o cache em disco de
    `costs_claude_transcript` (Task 1), não este módulo."""
    cfg = tmp_path / ".claude"
    _transcript_claude(cfg, "s", "claude-opus-5", "/r", i=1, o=0)
    _escrever(tmp_path / "sessions" / "--r--" / "s.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3"},
        {"type": "message", "message": {"usage": {"input": 1, "output": 1,
                                                  "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "sem-omp")
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "sem-codex")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "sem-kimi")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [(str(cfg), "c")])
    cs.invalidar_cache()   # senão o estado de um teste anterior contamina a contagem

    chamadas = {"claude": 0, "pi": 0}
    claude_original, pi_original = cs.linhas_claude, cs.linhas_pi

    def claude_contado(*a, **kw):
        chamadas["claude"] += 1
        return claude_original(*a, **kw)

    def pi_contado(*a, **kw):
        chamadas["pi"] += 1
        return pi_original(*a, **kw)

    monkeypatch.setattr(cs, "linhas_claude", claude_contado)
    monkeypatch.setattr(cs, "linhas_pi", pi_contado)

    cs.coletar()
    cs.coletar()
    assert chamadas == {"claude": 2, "pi": 1}, "pi cacheia por assinatura; claude roda a cada coletar()"


def test_linhas_omp_le_a_raiz_do_omp_com_source_omp_e_model_change_novo(tmp_path, monkeypatch):
    # omp grava model_change como "provider/id" (Pi grava provider e modelId separados).
    raiz = tmp_path / "omp" / "sessions" / "--w--"
    raiz.mkdir(parents=True)
    f = raiz / "2026-09-03T15-51-00-640Z_01a067f7-6120-700b-b71d-6a6092e0c720.jsonl"
    _escrever(f, [
        {"type": "session", "version": 3, "id": "01a067f7", "timestamp": "2026-09-03T15:51:00.640Z", "cwd": "/w"},
        {"type": "model_change", "model": "opencode-go/deepseek-v4-flash", "resolvedModelIsFallback": False},
        {"type": "message", "message": {"role": "assistant", "content": [], "usage": {"input": 10, "output": 5, "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "omp" / "sessions")
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "pi-vazia")
    linhas = cs.linhas_omp()
    assert len(linhas) == 1 and linhas[0].source == "omp"
    assert linhas[0].model == "deepseek-v4-flash"
    assert cs.linhas_pi() == []


def test_coletar_nao_conta_duas_vezes_quando_as_raizes_coincidem(tmp_path, monkeypatch):
    # PI_CODING_AGENT_DIR é do pi-coding-agent: exportada, os dois agentes caem na mesma árvore.
    raiz = tmp_path / "sessions"
    raiz.mkdir()
    monkeypatch.setattr(cs, "raiz_pi", lambda: raiz)
    monkeypatch.setattr(cs, "raiz_omp", lambda: raiz)
    monkeypatch.setattr(cs, "raiz_codex", lambda home=None: tmp_path / "x")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "y")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [])
    vistos = []
    monkeypatch.setattr(cs, "linhas_pi", lambda raiz=None, source="pi": vistos.append(source) or [])
    monkeypatch.setattr(cs, "linhas_omp", lambda: vistos.append("omp") or [])
    cs.coletar()
    assert vistos == ["pi"]


# --- Vieram do test_costs.py: o código que eles cobrem mudou de módulo, o risco não mudou -----


def test_claude_arquivo_ausente_devolve_vazio(tmp_path):
    assert cs.linhas_claude(tmp_path, "conta-teste") == []


def test_account_info_reads_oauth(tmp_path):
    (tmp_path / ".claude.json").write_text(json.dumps(
        {"oauthAccount": {"accountUuid": "u-9", "emailAddress": "x@y.com"}}))
    aid, email, label = cs.account_info(tmp_path, "fallback")
    assert (aid, email, label) == ("u-9", "x@y.com", "x@y.com")


def test_account_info_fallback_when_missing(tmp_path, monkeypatch):
    # config_dir sem .claude.json E HOME isolado (sem ~/.claude.json) -> cai no fallback.
    monkeypatch.setenv("HOME", str(tmp_path))
    aid, email, label = cs.account_info(tmp_path / "cfg", "fallback")
    assert aid == "fallback"
    assert email is None


def test_account_info_fallback_when_json_root_not_dict(tmp_path, monkeypatch):
    # .claude.json corrompido (root e lista, nao dict) -> nao explode, cai no fallback.
    monkeypatch.setenv("HOME", str(tmp_path))
    (tmp_path / ".claude.json").write_text("[1, 2, 3]")
    aid, email, label = cs.account_info(tmp_path, "fallback")
    assert aid == "fallback"
    assert email is None
    assert label == "fallback"


def test_claude_le_do_transcript_e_nao_do_plugin(tmp_path, monkeypatch):
    """O costs.jsonl do plugin ECC não pode mais ser a fonte: o app não pode depender de
    plugin de terceiro, e o resumo não enxerga subagente (medido: numa sessão com 14, o
    plugin registrou exatamente o pai e ignorou 12,8 M de cache lido dos filhos)."""
    from app import costs_claude_transcript as ct
    proj = tmp_path / ".claude" / "projects" / "p"
    proj.mkdir(parents=True)
    (proj / "s9.jsonl").write_text(json.dumps({
        "type": "assistant", "timestamp": "2026-08-01T10:00:00Z", "sessionId": "s9",
        "cwd": "/repo/novo", "message": {"model": "claude-opus-5", "usage": {
            "input_tokens": 11, "output_tokens": 2,
            "cache_creation_input_tokens": 3, "cache_read_input_tokens": 4}}}), encoding="utf-8")
    _escrever(tmp_path / ".claude" / "metrics" / "costs.jsonl", [])   # plugin vazio de propósito
    ct.invalidar_cache()
    linhas = cs.linhas_claude(tmp_path / ".claude", "anthropic:teste")
    assert len(linhas) == 1
    r = linhas[0]
    assert (r.input, r.output, r.cache_write, r.cache_read) == (11, 2, 3, 4)
    assert r.project == "/repo/novo"        # o cwd vem do próprio transcript
    assert r.subagente is False


def test_subagente_vira_linha_com_a_marca(tmp_path, monkeypatch):
    """13,7% do volume desta máquina. Tem que aparecer e tem que ser distinguível."""
    from app import costs_claude_transcript as ct
    proj = tmp_path / ".claude" / "projects" / "p"
    (proj / "abc" / "subagents").mkdir(parents=True)
    def t(i):
        return json.dumps({"type": "assistant", "timestamp": "2026-08-01T10:00:00Z",
                           "sessionId": "s1", "cwd": "/r", "message": {
                               "model": "claude-opus-5", "usage": {
                                   "input_tokens": i, "output_tokens": 0,
                                   "cache_creation_input_tokens": 0,
                                   "cache_read_input_tokens": 0}}})
    (proj / "abc.jsonl").write_text(t(100), encoding="utf-8")
    (proj / "abc" / "subagents" / "agent-x.jsonl").write_text(t(7), encoding="utf-8")
    _escrever(tmp_path / ".claude" / "metrics" / "costs.jsonl", [])
    ct.invalidar_cache()
    linhas = sorted(cs.linhas_claude(tmp_path / ".claude", "c"), key=lambda r: r.input)
    assert [r.subagente for r in linhas] == [True, False]
    assert linhas[0].session_id != linhas[1].session_id


def test_duas_contas_nao_contam_em_dobro(tmp_path, monkeypatch):
    """`coletar()` chama o leitor uma vez por config dir. Um leitor que ignorasse o argumento
    leria a MESMA raiz duas vezes e dobraria o gasto, dividido entre contas erradas."""
    from app import costs_claude_transcript as ct
    for conta in ("A", "B"):
        p = tmp_path / conta / "projects" / "p"
        p.mkdir(parents=True)
        (p / f"s-{conta}.jsonl").write_text(json.dumps({
            "type": "assistant", "timestamp": "2026-08-01T10:00:00Z", "sessionId": conta,
            "cwd": "/r", "message": {"model": "claude-opus-5", "usage": {
                "input_tokens": 1, "output_tokens": 0,
                "cache_creation_input_tokens": 0, "cache_read_input_tokens": 0}}}),
            encoding="utf-8")
    ct.invalidar_cache()
    a = cs.linhas_claude(tmp_path / "A", "conta-a")
    b = cs.linhas_claude(tmp_path / "B", "conta-b")
    assert len(a) == 1 and len(b) == 1
    assert a[0].session_id != b[0].session_id or a[0].provider != b[0].provider


def test_provedor_clinepass_nao_vira_moonshot():
    """Cline Pass é gateway de modelo MISTURADO — o repo documenta cline-pass/glm-5.2 em
    app/pi_models.py. Mapear pelo modelo de hoje inventaria a origem de amanhã, que é o mesmo
    motivo de openrouter e cline terem ficado de fora."""
    from app import pricing
    assert pricing.canonizar_provedor("clinepass") == "clinepass"
    assert pricing.canonizar_provedor("cline-pass") == "cline-pass"
    assert pricing.canonizar_provedor("kimi-coding") == "moonshotai"
    assert pricing.canonizar_provedor("openai-codex") == "openai"


def test_linhas_kimi_soma_usage_record_e_resolve_projeto(tmp_path, monkeypatch):
    # Layout medido no Kimi 0.34.0: sessions/<wd>/<sid>/agents/main/wire.jsonl, um usage.record
    # por turno com o DELTA (soma, nao ultima linha). Projeto vem do session_index.jsonl.
    home = tmp_path / "kimi-home"
    monkeypatch.setenv("KIMI_CODE_HOME", str(home))
    monkeypatch.setattr(cs, "raiz_kimi", lambda: home / "sessions")
    sid = "session_aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"
    wire = home / "sessions" / "wd_r_000000000000" / sid / "agents" / "main" / "wire.jsonl"
    _escrever(wire, [
        {"type": "metadata", "protocol_version": "1.5"},
        {"type": "usage.record", "model": "apikey/k3", "time": 1786452188171,
         "usage": {"inputOther": 100, "output": 10, "inputCacheRead": 50, "inputCacheCreation": 5}},
        {"type": "usage.record", "model": "apikey/k3", "time": 1786452190000,
         "usage": {"inputOther": 200, "output": 20, "inputCacheRead": 0, "inputCacheCreation": 0}},
    ])
    _escrever(home / "session_index.jsonl", [
        {"sessionId": sid, "sessionDir": str(wire.parent.parent), "workDir": "/r"}])
    linhas = cs.linhas_kimi()
    assert len(linhas) == 1
    r = linhas[0]
    assert r.source == "kimi"
    assert r.model == "apikey/k3"
    assert (r.input, r.output, r.cache_read, r.cache_write) == (300, 30, 50, 5)
    assert r.project == "/r"
    assert r.session_id == sid
    assert r.subagente is False


def test_linhas_kimi_marca_subagente(tmp_path, monkeypatch):
    home = tmp_path / "kimi-home"
    monkeypatch.setenv("KIMI_CODE_HOME", str(home))
    monkeypatch.setattr(cs, "raiz_kimi", lambda: home / "sessions")
    sid = "session_bbbbbbbb-cccc-dddd-eeee-ffffffffffff"
    _escrever(home / "sessions" / "wd_r_000000000000" / sid / "agents" / "agent-0" / "wire.jsonl", [
        {"type": "usage.record", "model": "apikey/k3", "time": 1786452188171,
         "usage": {"inputOther": 1, "output": 1, "inputCacheRead": 0, "inputCacheCreation": 0}},
    ])
    (r,) = cs.linhas_kimi()
    assert r.subagente is True


def test_coletar_codex_separa_contas_e_deduplica_rollout_canonico(tmp_path, monkeypatch):
    monkeypatch.setattr(__import__("pathlib").Path, "home",
                        classmethod(lambda cls: tmp_path))
    monkeypatch.setattr(codex_contas, "_DEFAULT_HOME", tmp_path / ".codex")
    default = codex_contas.Account("default", tmp_path / ".codex", True)
    work = codex_contas.create_account("work")
    sid_a = "a"
    sid_b = "b"
    original = default.home / "sessions" / "2026" / "09" / "09" / "rollout-a.jsonl"
    _escrever(original, _rollout_codex("/a", sid_a,
                                       {"input_tokens": 10, "output_tokens": 1}))
    other = work.home / "sessions" / "2026" / "09" / "09" / "rollout-b.jsonl"
    _escrever(other, _rollout_codex("/b", sid_b,
                                    {"input_tokens": 20, "output_tokens": 2}))
    link = work.home / "archived_sessions" / "rollout-a.jsonl"
    link.parent.mkdir(parents=True)
    link.symlink_to(original)
    monkeypatch.setattr(cs, "_config_dirs", lambda: [])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "pi")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "omp")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "kimi")
    cs.invalidar_cache()

    rows = cs.coletar()
    assert {row.session_id for row in rows if row.source == "codex"} == {sid_a, sid_b}
    assert sum(row.input for row in rows if row.source == "codex") == 30
    assert {row.account_id for row in rows if row.source == "codex"} == {
        f"codex:{default.home.resolve()}", f"codex:{work.home.resolve()}"
    }
    assert {row.provider for row in rows if row.source == "codex"} == {
        f"codex:{default.home.resolve()}", f"codex:{work.home.resolve()}"
    }


def test_coletar_link_da_padrao_para_secundaria_preserva_dono_e_cache(tmp_path, monkeypatch):
    default = codex_contas.Account("default", tmp_path / ".codex", True)
    work = codex_contas.create_account("work")
    rollout = work.home / "sessions" / "2026" / "09" / "09" / "rollout-only.jsonl"
    _escrever(rollout, _rollout_codex("/work", "only",
                                      {"input_tokens": 10, "output_tokens": 1}))
    monkeypatch.setattr(cs, "_config_dirs", lambda: [])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "pi")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "omp")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "kimi")
    cs.invalidar_cache()

    def codex_rows():
        return [row for row in cs.coletar() if row.source == "codex"]

    first = codex_rows()
    link = default.home / "sessions" / "2026" / "09" / "09" / "rollout-link.jsonl"
    link.parent.mkdir(parents=True)
    link.symlink_to(rollout)
    second = codex_rows()
    link.unlink()
    third = codex_rows()

    assert [(row.session_id, row.account_id, row.input) for row in first] == \
        [("only", f"codex:{work.home.resolve()}", 10)]
    assert [(row.session_id, row.account_id, row.input) for row in second] == \
        [("only", f"codex:{work.home.resolve()}", 10)]
    assert [(row.session_id, row.account_id, row.input) for row in third] == \
        [("only", f"codex:{work.home.resolve()}", 10)]


def test_coletar_codex_gateway_nao_atribui_custo_a_conta_openai(tmp_path, monkeypatch):
    home = tmp_path / ".codex"
    linhas = _rollout_codex("/r", "gateway", {"input_tokens": 10, "output_tokens": 1})
    linhas[0]["payload"]["model_provider"] = "gateway-proprio"
    _escrever(home / "sessions" / "rollout-gateway.jsonl", linhas)
    monkeypatch.setattr(cs, "_config_dirs", lambda: [])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "pi")
    monkeypatch.setattr(cs, "raiz_omp", lambda: tmp_path / "omp")
    monkeypatch.setattr(cs, "raiz_kimi", lambda: tmp_path / "kimi")
    (row,) = cs.coletar()
    assert row.provider == "gateway-proprio"
    assert row.account_id == f"codex:{home.resolve()}"
