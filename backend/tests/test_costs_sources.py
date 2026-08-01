import json
import os
from datetime import timezone

import pytest

from app import costs_sources as cs
from app import pricing


@pytest.fixture(autouse=True)
def _pricing_isolado(tmp_path, monkeypatch):
    """Os leitores resolvem PROVEDOR via pricing, então sem isto o teste lê o cache real em
    ~/.claude/.claude-pocket-pricing/ e passa ou falha conforme o estado da máquina — não do
    código. Mesma fixture do test_pricing.py."""
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")
    # getattr: `cs.invalidar_cache` só nasce na Task 6; até lá o no-op mantém a fixture válida
    # desde a Task 3, sem precisar reescrevê-la depois.
    limpar = lambda: getattr(cs, "invalidar_cache", lambda: None)()  # noqa: E731
    pricing.invalidar_cache(); limpar()
    yield
    pricing.invalidar_cache(); limpar()


def _escrever(p, linhas):
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) if isinstance(x, (dict, list)) else x for x in linhas))


def test_ler_jsonl_pula_linha_invalida_e_nao_dict(tmp_path):
    p = tmp_path / "x.jsonl"
    _escrever(p, [{"a": 1}, "{quebrado", "null", "[1,2]", {"b": 2}])
    assert list(cs._ler_jsonl(p)) == [{"a": 1}, {"b": 2}]


def test_cwd_sai_de_dentro_do_transcript(tmp_path):
    # O nome do diretório do Claude não é invertível ('Á' e espaço viram '-', igual à barra).
    # O cwd real está DENTRO do arquivo.
    t = tmp_path / "-home-jeff--rea-de-trabalho-x" / "s.jsonl"
    _escrever(t, [{"type": "last-prompt"}, {"type": "x"},
                  {"type": "user", "cwd": "/home/jeff/Área de trabalho/x"}])
    assert cs.cwd_do_transcript(str(t)) == "/home/jeff/Área de trabalho/x"


def test_cwd_ausente_vira_string_vazia(tmp_path):
    t = tmp_path / "s.jsonl"
    _escrever(t, [{"type": "last-prompt"}])
    assert cs.cwd_do_transcript(str(t)) == ""


def test_claude_pega_a_ultima_linha_por_sessao(tmp_path):
    # O costs.jsonl é CUMULATIVO: o hook grava um snapshot do total a cada turno.
    cfg = tmp_path / ".claude"
    t = cfg / "projects" / "-p" / "s1.jsonl"
    _escrever(t, [{"type": "user", "cwd": "/repo/um"}])
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s1", "transcript_path": str(t),
         "model": "claude-opus-5", "input_tokens": 10, "output_tokens": 1,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
        {"timestamp": "2026-08-01T11:00:00Z", "session_id": "s1", "transcript_path": str(t),
         "model": "claude-opus-5", "input_tokens": 99, "output_tokens": 9,
         "cache_write_tokens": 2, "cache_read_tokens": 3},
    ])
    linhas = cs.linhas_claude(cfg, "conta-x")
    assert len(linhas) == 1
    r = linhas[0]
    assert (r.input, r.output, r.cache_write, r.cache_read) == (99, 9, 2, 3)
    assert r.source == "claude"
    assert r.project == "/repo/um"
    assert r.provider == "conta-x"


def test_claude_ignora_synthetic(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "<synthetic>",
         "input_tokens": 1, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "conta-x") == []


def test_sessao_de_motor_ganha_provedor_do_modelo(tmp_path):
    # CP_ENGINE só existe em /proc de sessão VIVA. Numa linha de ontem, quem entrega o
    # provedor é o modelo: 'k3' só existe na Moonshot.
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "k3",
         "input_tokens": 5, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "conta-x")[0].provider == "moonshotai"


def test_transcript_sem_cwd_vira_projeto_desconhecido(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "transcript_path": "/nao/existe",
         "model": "claude-opus-5", "input_tokens": 1, "output_tokens": 1,
         "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    assert cs.linhas_claude(cfg, "c")[0].project == cs.PROJETO_DESCONHECIDO


def test_timestamp_vira_tz_aware(tmp_path):
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "claude-opus-5",
         "input_tokens": 1, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    ts = cs.linhas_claude(cfg, "c")[0].ts
    assert ts.tzinfo is not None
    assert ts.astimezone(timezone.utc).hour == 10


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
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sessions")
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
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "nao-existe")
    assert cs.linhas_codex() == []


def test_codex_rollout_sem_token_count_e_pulado(tmp_path, monkeypatch):
    raiz = tmp_path / "sessions" / "2026" / "08" / "01"
    _escrever(raiz / "rollout-x.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "type": "session_meta",
         "payload": {"cwd": "/r", "model_provider": "openai"}},
    ])
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sessions")
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
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sessions")
    sids = {r.session_id for r in cs.linhas_codex()}
    assert sids == {"viva", "arquivada"}


def test_codex_sem_archived_sessions_nao_quebra(tmp_path, monkeypatch):
    _escrever(tmp_path / "sessions" / "rollout-viva.jsonl",
              _rollout_codex("/repo/viva", "viva", {"input_tokens": 10, "output_tokens": 1}))
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sessions")
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
    assert pai.provider == "kimi-coding"
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
    assert r.provider == "openai-codex"
    assert r.model == "gpt-5.6-sol"


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
    monkeypatch.setattr(cs, "raiz_codex", lambda: raiz)
    assert cs.linhas_codex()[0].model == "?"


def test_coletar_junta_as_tres_fontes_e_dedup_e_por_fonte(tmp_path, monkeypatch):
    # Chave de dedup é (fonte, session_id): um uuid repetido entre fontes não pode se comer.
    cfg = tmp_path / ".claude"
    _escrever(cfg / "metrics" / "costs.jsonl", [
        {"timestamp": "2026-08-01T10:00:00Z", "session_id": "mesmo-id", "model": "claude-opus-5",
         "input_tokens": 1, "output_tokens": 1, "cache_write_tokens": 0, "cache_read_tokens": 0},
    ])
    _escrever(tmp_path / "sessions" / "--r--" / "mesmo-id.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3"},
        {"type": "message", "message": {"usage": {"input": 2, "output": 2,
                                                  "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sem-codex")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [(str(cfg), "conta-x")])
    cs.invalidar_cache()
    linhas = cs.coletar()
    assert {r.source for r in linhas} == {"claude", "pi"}
    assert len(linhas) == 2


def test_cache_relê_quando_o_arquivo_muda(tmp_path, monkeypatch):
    cfg = tmp_path / ".claude"
    arq = cfg / "metrics" / "costs.jsonl"
    linha = {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "claude-opus-5",
             "input_tokens": 1, "output_tokens": 0, "cache_write_tokens": 0,
             "cache_read_tokens": 0}
    _escrever(arq, [linha])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "x")
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "y")
    monkeypatch.setattr(cs, "_config_dirs", lambda: [(str(cfg), "c")])
    cs.invalidar_cache()
    assert len(cs.coletar()) == 1
    _escrever(arq, [linha, {**linha, "session_id": "s2"}])
    os.utime(arq, (0, 0))          # mtime diferente -> tem que reler
    assert len(cs.coletar()) == 2


def test_cache_evita_reparse_quando_nada_muda(tmp_path, monkeypatch):
    # O teste acima só prova "arquivo mudou -> relê". Isso passaria até sem cache nenhum. O que
    # falta é o lado que É o valor da tarefa: sem tocar arquivo, o parser roda UMA vez só.
    # Conta chamada real ao leitor de cada fonte, não só o tamanho do resultado.
    cfg = tmp_path / ".claude"
    arq = cfg / "metrics" / "costs.jsonl"
    linha = {"timestamp": "2026-08-01T10:00:00Z", "session_id": "s", "model": "claude-opus-5",
             "input_tokens": 1, "output_tokens": 0, "cache_write_tokens": 0,
             "cache_read_tokens": 0}
    _escrever(arq, [linha])
    _escrever(tmp_path / "sessions" / "--r--" / "s.jsonl", [
        {"type": "session", "timestamp": "2026-08-01T10:00:00Z", "cwd": "/r"},
        {"type": "model_change", "provider": "kimi-coding", "modelId": "k3"},
        {"type": "message", "message": {"usage": {"input": 1, "output": 1,
                                                  "cacheRead": 0, "cacheWrite": 0}}},
    ])
    monkeypatch.setattr(cs, "raiz_pi", lambda: tmp_path / "sessions")
    monkeypatch.setattr(cs, "raiz_codex", lambda: tmp_path / "sem-codex")
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
    assert chamadas == {"claude": 1, "pi": 1}, "duas coletas sem tocar arquivo -> parser roda 1x"

    _escrever(arq, [linha, {**linha, "session_id": "s2"}])
    os.utime(arq, (0, 0))
    cs.coletar()
    assert chamadas == {"claude": 2, "pi": 1}, "só o costs.jsonl mudou -> só o claude releu"
