"""Cobertura do sidecar de fila durável (pqueue): append/load, clear, e rename (move preservando
entradas). Isola o queue dir apontando settings.projects_dir pra um tmp."""
import os

import pytest

from app import pqueue
from app.pqueue import PromptQueue


@pytest.fixture(autouse=True)
def _tmp_queue_dir(tmp_path, monkeypatch):
    # _queue_dir() = settings.projects_dir.parent / ".hangar-queue" -> redireciona pro tmp.
    monkeypatch.setattr(pqueue.settings, "projects_dir", tmp_path / "projects")
    return tmp_path


def test_append_and_load_roundtrip():
    q = PromptQueue("s")
    q.append("um")
    q.append("dois")
    assert [e["text"] for e in PromptQueue("s").load()] == ["um", "dois"]


def test_clear_removes_sidecar():
    q = PromptQueue("s")
    q.append("x")
    q.clear()
    assert PromptQueue("s").load() == []


def test_bump_attempts_incrementa_e_devolve():
    q = PromptQueue("cc")
    e = q.append("oi")
    assert q.bump_attempts(e["id"]) == 1
    assert q.bump_attempts(e["id"]) == 2
    assert q.bump_attempts("id-que-nao-existe") == 0


def test_rename_moves_entries_and_drops_old():
    PromptQueue("old").append("msg um")
    PromptQueue("old").append("msg dois")
    PromptQueue("old").rename("new")
    assert PromptQueue("old").load() == []  # nome velho ficou vazio
    assert [e["text"] for e in PromptQueue("new").load()] == ["msg um", "msg dois"]


def test_rename_without_queue_is_noop():
    # Sessao sem fila: rename nao deve criar nada nem estourar.
    PromptQueue("sem-fila").rename("destino")
    assert PromptQueue("destino").load() == []


def test_append_default_pending_and_eager_delivered():
    PromptQueue("s").append("pendente")
    PromptQueue("s").append("eager", delivered=True)
    rows = PromptQueue("s").load()
    assert rows[0]["delivered"] is False
    assert rows[1]["delivered"] is True


def test_claim_undelivered_flips_and_is_idempotent():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=True)
    claimed = PromptQueue("s").claim_undelivered()
    assert [c["text"] for c in claimed] == ["a"]              # so a pendente
    assert all(r["delivered"] for r in PromptQueue("s").load())
    assert PromptQueue("s").claim_undelivered() == []          # 2a vez: nada (idempotente)


def test_claim_limit_one():
    PromptQueue("s").append("a", delivered=False)
    PromptQueue("s").append("b", delivered=False)
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["a"]
    assert [c["text"] for c in PromptQueue("s").claim_undelivered(limit=1)] == ["b"]


def test_claim_respects_min_ts():
    e = PromptQueue("s").append("antiga", delivered=False)
    assert PromptQueue("s").claim_undelivered(min_ts=e["ts"] + 1000) == []
    assert PromptQueue("s").load()[0]["delivered"] is False     # nao reivindicada


def test_claim_ignores_legacy_entry_without_key():
    # Entrada legada (escrita antes do campo): `is False` ESTRITO -> NAO reivindicada (senao um
    # upgrade re-enviaria todo prompt antigo ja entregue).
    p = PromptQueue("s")
    p.path.write_text('{"id":"old1","text":"legada","ts":1.0}\n', encoding="utf-8")
    assert p.claim_undelivered() == []
    assert "delivered" not in p.load()[0]


def test_set_delivered_reverts():
    e = PromptQueue("s").append("x", delivered=True)
    PromptQueue("s").set_delivered(e["id"], False)
    assert PromptQueue("s").load()[0]["delivered"] is False


def test_merged_history_dedup_is_ts_aware(tmp_path):
    # Texto REPETIDO: entrada enfileirada DEPOIS do commit de um texto igual NAO e absorvida por ele
    # (senao o 2o "ok" sumia do historico); a entrada anterior ao commit e absorvida (fluxo normal).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                    "message": {"role": "user", "content": "inicio"}}) + "\n" +
        json.dumps({"type": "user", "uuid": "u1", "timestamp": "2026-01-01T00:01:40Z",
                    "message": {"role": "user", "content": "ok"}}) + "\n",
        encoding="utf-8")
    tc = pqueue._ts_of_line(j.read_text(encoding="utf-8").splitlines()[1])  # epoch do commit de "ok"
    q = PromptQueue("s")
    q.path.write_text(
        json.dumps({"id": "e1", "text": "ok", "ts": tc - 5, "delivered": True}) + "\n" +
        json.dumps({"id": "e2", "text": "ok", "ts": tc + 5, "delivered": False}) + "\n",
        encoding="utf-8")
    ids = [e.id for e in pqueue.merged_history("s", str(j))]
    assert "queued-e1" not in ids      # anterior ao commit -> absorvida pelo user_msg real
    assert "queued-e2" in ids          # posterior ao commit -> ainda pendente, nao some


def test_merged_history_dedup_ts_race(tmp_path, monkeypatch):
    # Corrida REAL do envio (regressao de eb0f303): o send_prompt digita o texto + Enter e o Claude
    # Code grava o prompt no jsonl NA HORA; o append da fila so roda depois (_send_one). Carimbando
    # o ts DENTRO do append, a entrada nascia ~ms DEPOIS do commit do proprio texto -> o dedup lia
    # "commit anterior = de outra msg igual" e mantinha a entrada pendente: a msg aparecia DUAS
    # vezes no historico ate o reconcile (>= 8.5s). A faixa e de MILISSEGUNDOS — o teste irmao
    # (test_merged_history_dedup_is_ts_aware) usa +-5s e nunca a toca. Aqui o ts vem do _send_one
    # de verdade, que e onde a ordem send->append vive.
    import json
    from datetime import datetime, timezone
    from types import SimpleNamespace
    import app.api as api

    j = tmp_path / "t.jsonl"
    # 1a linha antiga = inicio da sessao: mantem start_ts no passado pra a poda pre-/clear nao ser
    # quem remove a entrada (o que deve absorve-la e o dedup, e e isso que este teste mede).
    j.write_text(json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "inicio"}}) + "\n",
                 encoding="utf-8")

    def fake_send_prompt(name, text, provider="claude", pane_id=None):
        # Espelha o Claude Code: o Enter do send_keys ja grava a entrada `user` no transcript.
        with open(j, "a", encoding="utf-8") as fh:
            fh.write(json.dumps({
                "type": "user", "uuid": "u1",
                "timestamp": datetime.now(timezone.utc).isoformat().replace("+00:00", "Z"),
                "message": {"role": "user", "content": text},
            }) + "\n")
        return "sent"

    monkeypatch.setattr(api.terminal, "send_prompt", fake_send_prompt)
    # Neutraliza o proprio Timer, nao so o _confirm_and_drain: result=="sent" constroi um
    # threading.Timer(8.5s) NAO-daemon de qualquer jeito — trocar o alvo por um no-op deixava o
    # interpretador esperando 8.5s pra sair (pytest reportava 0.26s, wall-clock 9.06s).
    monkeypatch.setattr(api.threading, "Timer", lambda *a, **k: SimpleNamespace(start=lambda: None))
    api._send_one("s", "JANELA-X")

    hist = pqueue.merged_history("s", str(j))
    assert [e.text for e in hist] == ["inicio", "JANELA-X"]   # uma bolha so, nao duas
    assert not any(e.id.startswith("queued-") for e in hist)  # entrada absorvida pelo user_msg real


def test_merged_history_ignores_delivered_flag(tmp_path):
    # delivered NAO afeta exibicao: entrada entregue mas ainda nao gravada no transcript continua
    # aparecendo como bubble queued- (o dedup por texto so a remove quando o user_msg real cai).
    j = tmp_path / "t.jsonl"
    j.write_text("", encoding="utf-8")
    PromptQueue("s").append("oi claude", delivered=True)
    hist = pqueue.merged_history("s", str(j))
    assert any(e.id.startswith("queued-") and e.text == "oi claude" for e in hist)


def test_merged_history_provider_claude_ignores_codex_rollout_shape(tmp_path):
    # Baseline: uma linha no shape do ROLLOUT do Codex (envelope response_item/payload) nao bate
    # com o parser do Claude (parse_obj espera obj["message"]) -> sem branch por provider isto
    # devolvia [] pra sessoes Codex (chat abria vazio ate o SSE encher via backfill do tail).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({
        "type": "response_item", "timestamp": "2026-01-01T00:00:00Z",
        "payload": {"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "oi"}]},
    }) + "\n", encoding="utf-8")
    assert pqueue.merged_history("s", str(j)) == []                    # default "claude"
    assert pqueue.merged_history("s", str(j), provider="claude") == []


def test_merged_history_provider_codex_parses_rollout_shape(tmp_path):
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({
        "type": "response_item", "timestamp": "2026-01-01T00:00:00Z",
        "payload": {"type": "message", "role": "user",
                    "content": [{"type": "input_text", "text": "oi"}]},
    }) + "\n", encoding="utf-8")
    hist = pqueue.merged_history("s", str(j), provider="codex")
    assert len(hist) == 1
    assert hist[0].kind == "user_msg"
    assert hist[0].text == "oi"


def test_prune_before_drops_previous_session_entries():
    q = PromptQueue("s")
    q.path.write_text(
        '{"id":"a","text":"velha","ts":10.0,"delivered":false}\n'
        '{"id":"b","text":"nova","ts":100.0,"delivered":false}\n', encoding="utf-8")
    q.prune_before(50.0)
    assert [r["id"] for r in q.load()] == ["b"]
    q.prune_before(0.0)                       # sem ts no transcript -> no-op seguro
    assert [r["id"] for r in q.load()] == ["b"]


def test_reconcile_confirms_requeues_and_silences_old():
    import json
    q = PromptQueue("s")
    rows = [
        {"id": "ok1", "text": "chegou", "ts": 900.0, "delivered": True},
        {"id": "gone", "text": "engolida", "ts": 900.0, "delivered": True},
        {"id": "fresh", "text": "recente", "ts": 999.0, "delivered": True},
        {"id": "old", "text": "pre-clear", "ts": 10.0, "delivered": True},
    ]
    q.path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    req = q.reconcile_delivered({"chegou"}, min_ts=100.0, now=1000.0)
    assert [r["id"] for r in req] == ["gone"]
    got = {r["id"]: r for r in q.load()}
    assert got["ok1"]["confirmed"] is True                       # no transcript -> confirmada
    assert got["gone"]["delivered"] is False and got["gone"]["attempts"] == 1  # re-drena
    assert "confirmed" not in got["fresh"]                       # dentro do grace: checa depois
    assert got["old"]["confirmed"] is True                       # sessao anterior: silenciada


def test_reconcile_gives_up_after_max_attempts(tmp_path):
    # "Desiste" NAO pode virar "confirmed": esse flag e o que faz merged_history/follow esconderem o
    # eco, porque significa "a bolha real do transcript ja cobre". Numa desistencia nao ha bolha
    # real — a msg foi engolida —, entao marcar confirmed SUMIA com a mensagem do usuario. O teste
    # antigo comentava "fica visivel" e nunca chamava merged_history pra provar; agora prova.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "inicio"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    # ts POSTERIOR ao inicio do transcript (2026-01-01), senao quem tira a bolha e a poda de
    # sessao-anterior do merged_history e o teste mediria a coisa errada.
    q.path.write_text('{"id":"x","text":"engolida","ts":1800000000.0,"delivered":true,"attempts":2}\n',
                      encoding="utf-8")
    assert q.reconcile_delivered(set(), min_ts=100.0, now=1800000100.0) == []
    row = q.load()[0]
    assert row["desistiu"] is True             # para de rechecar (sem loop de redigitacao)
    assert "confirmed" not in row              # ...mas NAO se passa por "achei no transcript"
    assert "engolida" in [e.text for e in pqueue.merged_history("s", str(j))]   # segue na tela


def test_reconcile_strips_attachment_marker():
    import json
    q = PromptQueue("s")
    q.path.write_text(json.dumps(
        {"id": "i", "text": "legenda — 📎 imagem: /x.png", "ts": 900.0, "delivered": True}
    ) + "\n", encoding="utf-8")
    assert q.reconcile_delivered({"legenda"}, min_ts=100.0, now=1000.0) == []
    assert q.load()[0]["confirmed"] is True    # transcript grava so a legenda -> casa sem o 📎


def test_committed_lines_include_queue_ops_and_raw_meta(tmp_path):
    # Mensagem entregue MID-TURN: (a) aparece embrulhada em meta na entrada user (o parser
    # descartaria) e (b) na fila interna do Claude Code (queue-operation) desde a digitacao.
    # As duas fontes contam como "aterrissou" — senao o reconcile redigitava msg ja recebida.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "message": {"role": "user",
                    "content": "<system-reminder>meta</system-reminder>\nmandada mid-turn"}}) + "\n" +
        json.dumps({"type": "queue-operation", "operation": "enqueue",
                    "content": "na fila interna"}) + "\n",
        encoding="utf-8")
    lines = pqueue.committed_user_lines(str(j))
    assert "mandada mid-turn" in lines
    assert "na fila interna" in lines


def test_merged_history_skips_confirmed_entries(tmp_path):
    # Entrada CONFIRMADA (texto comprovado no transcript pelo reconcile) nao vira bolha nunca mais
    # — nem no history nem no follow (mesmo flag) — mesmo que o dedup por texto nao a alcance.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text("", encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(json.dumps(
        {"id": "c1", "text": "ja coberta", "ts": 900.0, "delivered": True, "confirmed": True}
    ) + "\n", encoding="utf-8")
    assert not any(e.id == "queued-c1" for e in pqueue.merged_history("s", str(j)))


def test_reconcile_confirms_attachment_message_against_raw_line(tmp_path):
    # Msg do app COM anexo e digitada com o marcador na MESMA linha; o transcript guarda a linha
    # inteira. A comparacao casa raw-com-raw E podado-com-podado — a versao que podava so o lado
    # da fila deixava msg com imagem orfa pra sempre -> redigitada (duplicatas so-com-anexo).
    import json
    j = tmp_path / "t.jsonl"
    full = "olha esse bug — 📎 imagem: /up/x.png"
    j.write_text(json.dumps({"type": "user", "message": {"role": "user", "content": full}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(json.dumps({"id": "a1", "text": full, "ts": 900.0, "delivered": True}) + "\n",
                      encoding="utf-8")
    committed = pqueue.committed_user_lines(str(j))
    assert q.reconcile_delivered(committed, min_ts=100.0, now=1000.0) == []   # confirma, nao requeua
    assert q.load()[0]["confirmed"] is True

def test_merged_history_limit_tail_read_e_sufixo_do_parse_completo(tmp_path, monkeypatch):
    # Tail-read (limit): parseia so o fim do arquivo, mas o resultado tem que ser um SUFIXO
    # identico ao parse completo. Janela encolhida via monkeypatch pra exercitar o crescimento 4x
    # (limit=50 nao cabe em 2KB -> cresce ate cobrir).
    import json
    j = tmp_path / "t.jsonl"
    lines = [
        json.dumps({"type": "user", "uuid": f"u{i}",
                    "timestamp": f"2026-01-01T00:{i // 60:02d}:{i % 60:02d}Z",
                    "message": {"role": "user", "content": f"msg {i} " + "x" * 100}})
        for i in range(200)
    ]
    j.write_text("\n".join(lines) + "\n", encoding="utf-8")
    full = [e.id for e in pqueue.merged_history("s", str(j))]
    monkeypatch.setattr(pqueue, "_TAIL_WINDOW", 2048)
    for lim in (5, 50):
        tail = [e.id for e in pqueue.merged_history("s", str(j), limit=lim)]
        assert len(tail) >= lim
        assert tail == full[-len(tail):]

def test_reconcile_confirma_msg_com_imagem_prefixo_image_n(tmp_path):
    # Claude Code grava prompt com anexo como "[Image #N]<texto> — 📎 imagem:" (prefixo prependado,
    # path removido). Sem normalizar o prefixo, a entrada delivered nunca confirmava contra o
    # transcript e era redigitada ate max_attempts (msg do app chegava 3x na sessao).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
                    "message": {"role": "user",
                                "content": [{"type": "text",
                                             "text": "[Image #1]olha isso — 📎 imagem:"}]}}) + "\n",
        encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(
        json.dumps({"id": "e1", "text": "olha isso — 📎 imagem: /tmp/x.png",
                    "ts": 100.0, "delivered": True}) + "\n",
        encoding="utf-8")
    requeued = q.reconcile_delivered(pqueue.committed_user_lines(str(j)), 0.0, now=1000.0)
    assert requeued == []                       # texto TA no transcript -> nao redigitar
    assert q.load()[0]["confirmed"] is True


def test_assistant_event_carrega_ts_e_janela_de_cache():
    """O turno do assistente leva a hora e a janela de cache MEDIDA (não suposta).

    O `usage.cache_creation` separa `ephemeral_1h_input_tokens` de `ephemeral_5m_input_tokens` —
    é de lá que sai o TTL. Sem esse detalhe, `cache_ttl_s` fica None: não mostrar prazo é melhor
    do que mostrar um prazo errado.
    """
    from app.transcript import parse_obj

    def linha(cache_creation):
        return {
            "type": "assistant",
            "uuid": "u1",
            "timestamp": "2026-07-26T11:02:15.023Z",
            "message": {
                "content": [{"type": "text", "text": "oi"}],
                "usage": {"cache_read_input_tokens": 471558, "cache_creation": cache_creation},
            },
        }

    (ev,) = parse_obj(linha({"ephemeral_1h_input_tokens": 190, "ephemeral_5m_input_tokens": 0}))
    assert ev.kind == "assistant_msg"
    assert ev.ts == pytest.approx(1785063735.023, abs=1)   # ISO -> epoch
    assert ev.cache_read == 471558
    assert ev.cache_ttl_s == 3600

    (cinco,) = parse_obj(linha({"ephemeral_1h_input_tokens": 0, "ephemeral_5m_input_tokens": 190}))
    assert cinco.cache_ttl_s == 300

    # Formato sem a quebra por TTL -> sem prazo, mas o resto continua vindo.
    (sem,) = parse_obj(linha(None))
    assert sem.cache_ttl_s is None
    assert sem.cache_read == 471558
    assert sem.ts is not None


def test_cache_info_nao_derruba_o_parse_com_valor_estranho():
    """Valor não-numérico em `cache_creation` não pode levantar.

    A exceção subia por parse_obj → tail → SSE e derrubava a conexão. Pior: o backfill relê as
    últimas linhas a cada reconexão, então UMA linha estranha viraria queda em loop pra sessão.
    """
    from app.transcript import parse_obj

    obj = {
        "type": "assistant",
        "uuid": "u1",
        "timestamp": "2026-07-26T11:02:15.023Z",
        "message": {
            "content": [{"type": "text", "text": "oi"}],
            "usage": {
                "cache_read_input_tokens": "nao-numero",
                "cache_creation": {"ephemeral_1h_input_tokens": "corrompido"},
            },
        },
    }
    (ev,) = parse_obj(obj)          # não levanta
    assert ev.cache_ttl_s is None   # sem TTL confiável -> sem prazo
    assert ev.cache_read is None
    assert ev.ts is not None        # o resto do evento continua útil


def test_timestamp_sem_fuso_e_lido_como_utc():
    """Sem sufixo de fuso, `.timestamp()` assumiria o fuso LOCAL do processo e devolveria um epoch
    deslocado, calado. O transcript escreve UTC."""
    from app.transcript import parse_obj

    def ev_de(ts_raw):
        (ev,) = parse_obj({
            "type": "assistant", "uuid": "u1", "timestamp": ts_raw,
            "message": {"content": [{"type": "text", "text": "oi"}]},
        })
        return ev

    assert ev_de("2026-07-26T11:02:15.023").ts == ev_de("2026-07-26T11:02:15.023Z").ts
    assert ev_de("nao-e-data").ts is None


def test_is_video_cobre_as_extensoes_servidas_pelo_app():
    from app.video import is_video

    for ext in ("mp4", "mov", "webm", "mkv", "m4v", "avi"):
        assert is_video(f"/tmp/a.{ext}"), ext
        assert is_video(f"/tmp/A.{ext.upper()}"), ext
    for ext in ("png", "jpg", "mp3", "pdf", ""):
        assert not is_video(f"/tmp/a.{ext}"), ext


# ---------------------------------------------------------------------------
# Surrogate solto no texto do usuário (meio emoji) — lado da ESCRITA
# ---------------------------------------------------------------------------

def test_append_com_surrogate_solto_grava_e_le_de_volta():
    # O browser fatia string por UNIDADE UTF-16: cortar um emoji ao meio antes do JSON.stringify
    # manda "\ud83d" sozinho. json.dumps aceita, mas o write_text estourava
    # UnicodeEncodeError -> POST /input virava 500 e a mensagem sumia sem rastro.
    q = PromptQueue("s")
    entry = q.append("corte \ud83d", delivered=True, ts=1.0)
    assert entry["text"] == "corte �"          # a entrada devolvida = a que foi pro disco
    raw = q.path.read_text(encoding="utf-8")        # <- este era o passo que levantava
    assert "\ud83d" not in raw
    assert PromptQueue("s").load()[0]["text"] == "corte �"


def test_append_preserva_emoji_bem_formado_byte_a_byte():
    # Bandeira (par de regional indicators), família (ZWJ) e emoji simples continuam CRUS no
    # arquivo (ensure_ascii=False preservado) e idênticos na volta.
    textos = ["bandeira 🇧🇷", "família 👨‍👩‍👧‍👦", "ok ✅"]
    q = PromptQueue("s")
    for t in textos:
        q.append(t)
    assert [e["text"] for e in PromptQueue("s").load()] == textos
    raw = q.path.read_bytes()
    for t in textos:
        assert t.encode("utf-8") in raw


def test_merged_history_absorve_entrada_com_anexo_pela_legenda(tmp_path):
    # Bug medido em 03/08/2026: msg COM IMAGEM aparecia DUAS vezes no chat (a bolha da fila, com as
    # miniaturas, e a real) durante todo o turno. O dedup comparava o texto CRU, e com anexo os dois
    # lados nunca batem: a fila guarda os paths numa linha so; o Claude Code quebra a linha depois de
    # cada "📎 imagem:", prefixa "[Image #N]" e CONSOME o path da imagem que virou anexo. So o
    # reconcile do idle desempatava -- tarde demais, e a pessoa esta olhando durante o turno.
    import json
    cap = "olha esse bug ai"
    fila = f"{cap} — 📎 imagem: /up/a.png 📎 imagem: /up/b.png"
    # Como o transcript grava: prefixo, quebra de linha e o ultimo path sumido (virou anexo real).
    real = f"[Image #1]{cap} — 📎 imagem:\n/up/a.png 📎 imagem:"
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                    "message": {"role": "user", "content": "inicio"}}) + "\n" +
        json.dumps({"type": "user", "uuid": "u1", "timestamp": "2026-01-01T00:01:40Z",
                    "message": {"role": "user", "content": [{"type": "text", "text": real},
                                                            {"type": "image", "source": {}}]}}) + "\n",
        encoding="utf-8")
    tc = pqueue._ts_of_line(j.read_text(encoding="utf-8").splitlines()[1])
    PromptQueue("s").path.write_text(
        json.dumps({"id": "e1", "text": fila, "ts": tc - 5, "delivered": True}) + "\n",
        encoding="utf-8")
    hist = pqueue.merged_history("s", str(j))
    assert not any(e.id.startswith("queued-") for e in hist), "bolha da fila duplicando a real"
    assert sum(1 for e in hist if e.kind == "user_msg" and cap in (e.text or "")) == 1


def test_merged_history_nao_absorve_legenda_igual_enviada_depois(tmp_path):
    # A folga do dedup por legenda tem limite: entrada enfileirada DEPOIS do commit de uma msg de
    # legenda igual segue pendente (mesma regra ts-aware do texto cru -- senao a 2a foto com a mesma
    # legenda sumia do chat).
    import json
    cap = "olha esse bug ai"
    j = tmp_path / "t.jsonl"
    j.write_text(
        json.dumps({"type": "user", "uuid": "u1", "timestamp": "2026-01-01T00:01:40Z",
                    "message": {"role": "user", "content": f"{cap} — 📎 imagem:\n/up/a.png"}}) + "\n",
        encoding="utf-8")
    tc = pqueue._ts_of_line(j.read_text(encoding="utf-8").splitlines()[0])
    PromptQueue("s").path.write_text(
        json.dumps({"id": "e2", "text": f"{cap} — 📎 imagem: /up/z.png", "ts": tc + 5,
                    "delivered": True}) + "\n",
        encoding="utf-8")
    assert any(e.id == "queued-e2" for e in pqueue.merged_history("s", str(j)))


def _wire_kimi(tmp_path, linhas):
    import json
    j = tmp_path / "wire.jsonl"
    j.write_text("\n".join(json.dumps(l) for l in linhas) + "\n", encoding="utf-8")
    return str(j)


def test_committed_user_lines_kimi_provider(tmp_path):
    # Sem o branch kimi o oraculo devolvia set() vazio e o reconcile redigitava cada entrega
    # (3x "ola" em producao, 2026-08-11). A injection NAO pode contar como texto do usuario.
    wire = _wire_kimi(tmp_path, [
        {"type": "context.append_message", "time": 1786453187986,
         "message": {"role": "user", "id": "msg_1", "origin": {"kind": "user"},
                     "content": [{"type": "text", "text": "ola"}]}},
        {"type": "context.append_message", "time": 1786453187990,
         "message": {"role": "user", "id": "msg_2", "origin": {"kind": "injection"},
                     "content": [{"type": "text", "text": "<system-reminder>x</system-reminder>"}]}},
    ])
    lines = pqueue.committed_user_lines(wire, "kimi")
    assert "ola" in lines
    assert not any("system-reminder" in l for l in lines)


def test_committed_user_lines_pi_e_omp_provider(tmp_path):
    # omp e o fork do Pi e usa o MESMO parser (app/adapters/pi/transcript.py) — sem cobertura direta
    # aqui, um shape novo do parser quebraria os dois calado.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "message", "id": "m1", "message": {
        "role": "user", "timestamp": 1, "content": [{"type": "text", "text": "oi"}]}}) + "\n",
        encoding="utf-8")
    assert pqueue.committed_user_lines(str(j), provider="omp") == {"oi"}
    assert pqueue.committed_user_lines(str(j), provider="pi") == {"oi"}


def test_transcript_start_ts_kimi_envelope_time(tmp_path):
    # O ts do Kimi mora no envelope `time` (ms) — sem isto o start_ts era 0.0 e a poda de fila
    # pre-/clear nao funcionava pro provider.
    wire = _wire_kimi(tmp_path, [
        {"type": "metadata", "protocol_version": "1.5", "created_at": 1786452160239},
        {"type": "context.append_message", "time": 1786453187986,
         "message": {"role": "user", "id": "m1", "origin": {"kind": "user"},
                     "content": [{"type": "text", "text": "oi"}]}},
    ])
    assert pqueue._transcript_start_ts(wire) == 1786452160.239


def test_merged_history_kimi_provider(tmp_path):
    # O /history passa o provider: sem o branch, o parser do Claude nao lia NENHUMA linha do wire
    # e o historico de uma sessao Kimi voltava vazio (so o SSE ao vivo enchia o chat).
    wire = _wire_kimi(tmp_path, [
        {"type": "context.append_message", "time": 1786453187986,
         "message": {"role": "user", "id": "m1", "origin": {"kind": "user"},
                     "content": [{"type": "text", "text": "ola"}]}},
        {"type": "context.append_loop_event", "time": 1786453190000,
         "event": {"type": "content.part", "uuid": "u1",
                   "part": {"type": "text", "text": "oi!"}}},
    ])
    evs = pqueue.merged_history("s", wire, "kimi")
    assert [(e.kind, e.text) for e in evs] == [("user_msg", "ola"), ("assistant_msg", "oi!")]


# --- Redigitacao as cegas: o incidente de 2026-08-11 (kill-server -> resume -> msg duplicada) ---
# Cadeia medida: um subagente rodou `tmux kill-server` 13:55:29 (o log do backend registra
# "no server running on /tmp/tmux-1000/default"); as sessoes morreram e a de nome `hangar` voltou
# 13:59:09 via `claude --resume`. A fila duravel e um arquivo por NOME de sessao, entao ela
# sobreviveu ao pane. As 14:00:48 saiu UM POST /input; as 14:01:48 o backend concluiu "a TUI
# engoliu" e redigitou (log REQUEUE n=1, e a entrada do sidecar com attempts:1) -> a mesma
# mensagem apareceu duas vezes no chat.
# O guard de `_confirm_and_drain` so adia enquanto o marcador do hook diz `working`. Sessao
# ressuscitada e exatamente o caso em que esse marcador nao e confiavel: `get_state` devolve None
# e o codigo caia PRA FRENTE, redigitando as cegas dentro de um turno vivo.
# Regra: redigitar e a acao destrutiva aqui (mete texto num prompt em uso). Sem PROVA de que a
# sessao nao esta no meio de um turno, nao se redigita — confirma e desiste. O pior caso vira o
# comportamento antigo (envio engolido fica visivel como bolha), que e falha VISIVEL, nao duplicata.

def _sessao_fake(nome, jsonl, provider="claude"):
    from types import SimpleNamespace
    return SimpleNamespace(name=nome, jsonl=str(jsonl), provider=provider)


def _cenario_engolida(tmp_path, monkeypatch, estado, provider="claude"):
    """Fila com uma entrega nao-confirmada + transcript SEM o texto. `estado` = o que o marcador
    do hook responde. Devolve (chamou_drain, linha_da_fila_depois)."""
    import json
    import time as _t
    from types import SimpleNamespace
    import app.api as api

    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "inicio"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("ressuscitada")
    q.path.write_text(json.dumps({"id": "e1", "text": "MENSAGEM-UNICA", "ts": _t.time() - 30,
                                  "delivered": True}) + "\n", encoding="utf-8")

    monkeypatch.setattr(api.registry, "list", lambda: [_sessao_fake("ressuscitada", j, provider)])
    monkeypatch.setattr(api.hook_state, "get_state", lambda _sid: estado)
    monkeypatch.setattr(api.threading, "Timer", lambda *a, **k: SimpleNamespace(start=lambda: None))
    chamou = []
    monkeypatch.setattr(api, "drain", lambda *a, **k: chamou.append(a))

    api._confirm_and_drain("ressuscitada")
    return chamou, q.load()[0]


def test_confirm_nao_redigita_com_estado_desconhecido(tmp_path, monkeypatch):
    # Marcador ausente (sessao ressuscitada apos o tmux morrer): NAO pode redigitar.
    chamou, row = _cenario_engolida(tmp_path, monkeypatch, None)
    assert chamou == []                        # nada de re-drenar -> nada de segunda digitacao
    assert row["delivered"] is True            # nunca volta pra fila
    assert not row.get("attempts")             # nao contou tentativa
    assert row["desistiu"] is True             # para de rechecar, SEM se passar por confirmada
    assert "confirmed" not in row              # senao a msg do usuario sumiria da tela


def test_confirm_ainda_redigita_com_estado_conhecido_ocioso(tmp_path, monkeypatch):
    # O contrario, pra a correcao acima nao matar a feature: estado PROVADAMENTE ocioso e
    # texto ausente do transcript continua sendo re-enfileirado (envio engolido pela TUI).
    import time as _t
    chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("idle", _t.time()))
    assert chamou and chamou[0][0] == "ressuscitada"
    assert row["delivered"] is False and row["attempts"] == 1


# 26/08/2026, relatado de uma maquina Windows: a mesma mensagem entrou 3x na conversa (18:27,
# 18:33, 18:37 — uma por fim de turno) e terminou com a tarja "nao chegou na sessao". Sao os 2
# requeues do reconcile + a desistencia. O oraculo de "chegou?" e `committed_user_lines`, e ele
# engolia OSError devolvendo o set montado ATE o erro: leitura que falha respondia "nada chegou",
# e isso autoriza redigitar. No Windows ler o .jsonl que o Claude Code esta escrevendo pode voltar
# WinError 32. Regra: oraculo que nao conseguiu ler nao decide nada.

def test_committed_user_lines_none_quando_nao_da_pra_ler(tmp_path):
    import json
    # Diretorio no lugar do arquivo: IsADirectoryError no Linux, PermissionError no Windows — os
    # dois sao OSError, que e a familia que o antigo `except` engolia.
    assert pqueue.committed_user_lines(str(tmp_path)) is None
    # E o contraste, pra "None" nao virar o retorno de tudo: arquivo legivel devolve set.
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "oi"}}) + "\n",
                 encoding="utf-8")
    assert pqueue.committed_user_lines(str(j)) == {"oi"}


def test_transcript_start_ts_none_quando_nao_da_pra_ler(tmp_path):
    # Irma da de cima, e o motivo de ela existir e o MESMO: 0.0 aqui significa "sem corte por
    # idade", e sem esse corte uma entrada de sessao ANTERIOR deixa de ser dispensada e cai no
    # caminho que redigita. "Nao consegui ler" nao pode se passar por "nao ha timestamp".
    assert pqueue._transcript_start_ts(str(tmp_path)) is None
    vazio = tmp_path / "sem-ts.jsonl"
    vazio.write_text("{}\n", encoding="utf-8")
    assert pqueue._transcript_start_ts(str(vazio)) == 0.0   # legivel e sem ts continua 0.0


def test_confirm_nao_decide_com_inicio_de_transcript_ilegivel(tmp_path, monkeypatch, caplog):
    # Mesmo cenario do requeue legitimo, mas com a SEGUNDA leitura do transcript falhando. Sem o
    # guard, min_ts=0.0 desliga a poda por idade e o reconcile decide com meia informacao.
    import logging
    import time as _t
    import app.api as api
    monkeypatch.setattr(api, "_transcript_start_ts", lambda *a, **k: None)
    with caplog.at_level(logging.WARNING, logger="hangar"):
        chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("idle", _t.time()))
    assert chamou == []
    assert row["delivered"] is True and not row.get("attempts")
    assert "desistiu" not in row and "confirmed" not in row
    assert "confirmacao adiada" in caplog.text


def test_confirm_nao_decide_com_transcript_ilegivel(tmp_path, monkeypatch, caplog):
    # MESMO cenario do teste de cima (estado provadamente ocioso + texto ausente), que redigita de
    # proposito — o que muda e so o oraculo nao ter conseguido ler. Aqui nao se toca na fila: a
    # entrada segue entregue-nao-confirmada e visivel como bolha, e o proximo fim de turno reolha.
    import logging
    import time as _t
    import app.api as api
    monkeypatch.setattr(api, "committed_user_lines", lambda *a, **k: None)
    with caplog.at_level(logging.WARNING, logger="hangar"):
        chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("idle", _t.time()))
    assert chamou == []                        # nada de re-drenar -> nada de segunda digitacao
    assert row["delivered"] is True
    assert not row.get("attempts")
    assert "desistiu" not in row and "confirmed" not in row
    # Tirar o guard NAO pode passar neste teste por acidente: sem ele o None desce ate o
    # reconcile, estoura TypeError e o `except Exception` de _confirm_and_drain engole — a fila
    # fica intacta pelo motivo ERRADO e as tres asserts acima continuariam verdes. O que separa os
    # dois e QUAL linha foi registrada.
    assert "confirmacao adiada" in caplog.text
    assert "confirmacao de entrega falhou" not in caplog.text


def test_linha_mais_parecida_aponta_o_quase_igual():
    # O log do REQUEUE precisa mostrar CONTRA O QUE a comparacao falhou. Caso desenhado a partir do
    # relato: o texto tem uma barra invertida a mais que a linha gravada no transcript.
    texto = r"pode gerar os scripts e colar no servidor (\\servidor\SQL\banco.sql)"
    committed = {r"pode gerar os scripts e colar no servidor (\servidor\SQL\banco.sql)",
                 "outra coisa completamente diferente"}
    assert pqueue.linha_mais_parecida(texto, committed) == \
        r"pode gerar os scripts e colar no servidor (\servidor\SQL\banco.sql)"
    # Sem nada parecido, None — melhor calar do que apontar uma linha aleatoria como "a candidata".
    assert pqueue.linha_mais_parecida(texto, {"nada a ver"}) is None
    assert pqueue.linha_mais_parecida("", committed) is None


def test_confirm_adia_enquanto_trabalha(tmp_path, monkeypatch):
    # Guard que ja existia: mid-turn nao se mexe na fila (nem confirma, nem redigita).
    import time as _t
    chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("working", _t.time()))
    assert chamou == []
    assert "confirmed" not in row and not row.get("attempts")


def _cenario_turno_longo(tmp_path, monkeypatch, com_texto, provider="claude"):
    """Entrega antiga nao-confirmada + marcador working + transcript COM (ou SEM) o texto.
    Devolve (chamou_drain, timers_agendados, linha_da_fila_depois)."""
    import json
    import time as _t
    from types import SimpleNamespace
    import app.api as api

    j = tmp_path / "t.jsonl"
    conteudo = "MENSAGEM-UNICA" if com_texto else "outra-coisa"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": conteudo}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("turno-longo")
    q.path.write_text(json.dumps({"id": "e1", "text": "MENSAGEM-UNICA", "ts": _t.time() - 30,
                                  "delivered": True}) + "\n", encoding="utf-8")

    monkeypatch.setattr(api.registry, "list", lambda: [_sessao_fake("turno-longo", j, provider)])
    monkeypatch.setattr(api.hook_state, "get_state", lambda _sid: ("working", _t.time()))
    timers = []
    class _TimerRec:
        def __init__(self, delay, target, args=()):
            timers.append((delay, target.__name__, args))
        def start(self):
            pass
    monkeypatch.setattr(api.threading, "Timer", _TimerRec)
    chamou = []
    monkeypatch.setattr(api, "drain", lambda *a, **k: chamou.append(a))

    api._confirm_and_drain("turno-longo")
    return chamou, timers, q.load()[0]


def test_turno_longo_confirma_prova_no_transcript(tmp_path, monkeypatch):
    # Sessao que trabalha HORAS sem ficar ociosa nunca chega no reconcile do idle: o guard de
    # mid-turn adiava pra sempre e o follow reemitia a fila inteira como bolha fantasma a cada
    # reconexao do SSE (bug medido: i18n-writer-t12 com 9 entradas delivered e confirmed:null).
    # Com o texto PROVADO no transcript, carimbar confirmed e seguro ate no meio do turno — nao
    # redigita, nao desiste: so esconde o eco que a bolha real ja cobre.
    chamou, timers, row = _cenario_turno_longo(tmp_path, monkeypatch, com_texto=True)
    assert row["confirmed"] is True            # carimbou sem esperar o ocioso
    assert "desistiu" not in row
    assert row["delivered"] is True
    assert chamou == []                        # nada de redigitar no meio do turno
    assert timers == []                        # nada pendente -> corrente de re-checagem parou


def test_turno_longo_nao_decide_engolida_sem_prova(tmp_path, monkeypatch):
    # O reverso, pro guard nao perder o motivo de existir: SEM o texto no transcript, no meio do
    # turno nao se decide NADA (nem desistiu — pode ainda estar na fila interna da TUI) — so
    # reagenda pra olhar de novo.
    chamou, timers, row = _cenario_turno_longo(tmp_path, monkeypatch, com_texto=False)
    assert "confirmed" not in row
    assert "desistiu" not in row
    assert row["delivered"] is True
    assert chamou == []
    assert timers and timers[0][1] == "_confirm_and_drain"


# 13/08/2026: a mesma mensagem entrou 3x na fila da TUI de uma sessao Kimi (REQUEUE n=3 no log das
# 08:29, tres bolhas identicas visiveis no pane). No Kimi um prompt digitado durante um turno fica
# na fila da TUI e so entra no wire.jsonl quando o turno chega nele — nao ha o `queue-operation` do
# Claude Code, que e o registro feito no momento da digitacao. Logo "ausente do transcript" nao
# prova engolido. E o marcador de estado nao salva: o Stop de um SUBAGENTE grava "idle" na chave do
# pai, entao o guard de working acima ja tinha soltado o caminho.
def test_confirm_nunca_redigita_no_kimi_mesmo_ocioso(tmp_path, monkeypatch):
    import time as _t
    chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("idle", _t.time()), provider="kimi")
    assert chamou == []                        # nada de re-drenar -> nunca uma segunda digitacao
    assert row["delivered"] is True
    assert not row.get("attempts")
    assert row["desistiu"] is True             # visivel como bolha da fila, nao escondida
    assert "confirmed" not in row


# `desistiu` e decidido DEPOIS que a entrada nasce (o reconcile roda num Timer, segundos mais tarde).
# Com um set de ids ja vistos, o follow emitia a entrada UMA vez — ainda sem o campo — e a virada pra
# "perdida" nunca chegava a quem esta com o chat ABERTO. So quem recarregava (novo /history) via o
# aviso, ou seja: o caminho mais comum era justamente o que nao mostrava nada.
def test_follow_reemite_quando_a_entrada_vira_perdida(tmp_path):
    # `desistiu` e decidido DEPOIS que a entrada nasce (o reconcile roda num Timer, segundos mais
    # tarde). Com um SET de ids ja vistos, o follow emitia a bolha UMA vez — ainda sem o campo — e a
    # virada pra "perdida" nunca chegava a quem esta com o chat ABERTO: so quem recarregava via o
    # aviso, ou seja, o caminho mais comum era justamente o que nao mostrava nada. Reemitir e seguro
    # porque o front indexa por id e SUBSTITUI no lugar (Chat.svelte, idIndex).
    import asyncio
    import json

    q = PromptQueue("reemite")
    linha = q.append("oi", delivered=True)

    async def duas_passadas():
        gen = q.follow()
        primeiro = await anext(gen)
        # o reconcile carimba `desistiu` — MESMA entrada, estado novo
        q.path.write_text(json.dumps({**linha, "delivered": True, "desistiu": True}) + "\n",
                          encoding="utf-8")
        segundo = await asyncio.wait_for(anext(gen), timeout=10)
        await gen.aclose()
        return primeiro, segundo

    primeiro, segundo = asyncio.run(duas_passadas())
    assert primeiro.id == segundo.id           # a MESMA bolha, atualizada — nao uma segunda
    assert primeiro.desistiu is None           # ao nascer, ainda nao se sabe
    assert segundo.desistiu is True            # e a virada chega a quem esta com o chat aberto


def test_entry_event_carrega_desistiu():
    # O campo tem que ATRAVESSAR o backend: sem ele no ChatEvent, a bolha perdida renderiza igual a
    # uma aceita e "sumiu sem aviso" vira "parece que foi" — pior que o proprio sumico.
    from app.pqueue import _entry_event
    assert _entry_event({"id": "e1", "text": "oi"}).desistiu is None
    assert _entry_event({"id": "e1", "text": "oi", "desistiu": True}).desistiu is True


def test_reconcile_resgata_desistida_que_apareceu_depois():
    # `desistiu` era irreversivel: a bolha ficava avisando "nao chegou" pra sempre sobre uma msg que
    # CHEGOU — so que depois do prazo. Medido em 13/08/2026 numa sessao Kimi: 6 de 7 desistidas
    # estavam no wire.jsonl no fim do dia. O reload escondia (merged_history absorve), o SSE ao vivo
    # nao — entao o usuario via o aviso errado ate recarregar.
    import json
    import time as _t
    q = PromptQueue("resgate")
    q.path.write_text(json.dumps({"id": "e1", "text": "chegou tarde", "ts": _t.time() - 100,
                                  "delivered": True, "desistiu": True}) + "\n", encoding="utf-8")
    q.reconcile_delivered({"chegou tarde"}, 0.0, _t.time())
    r = q.load()[0]
    assert r.get("confirmed") is True         # vira aceita
    assert "desistiu" not in r                # e para de avisar "nao chegou"


def test_reconcile_mantem_desistida_que_nao_apareceu():
    # O contrario, pra o resgate nao apagar falha de verdade: texto ausente do transcript continua
    # marcado como perdido.
    import json
    import time as _t
    q = PromptQueue("resgate2")
    q.path.write_text(json.dumps({"id": "e1", "text": "sumiu mesmo", "ts": _t.time() - 100,
                                  "delivered": True, "desistiu": True}) + "\n", encoding="utf-8")
    q.reconcile_delivered({"outra coisa"}, 0.0, _t.time())
    r = q.load()[0]
    assert r.get("desistiu") is True
    assert "confirmed" not in r


def test_resgate_nao_confirma_duas_entradas_com_a_mesma_linha():
    # `committed` e um SET: duas entradas com o MESMO texto (comum em resposta de picker —
    # "Respondendo à pergunta: Sim" se repete) casariam as duas contra a MESMA linha do transcript.
    # A que se perdeu de verdade viraria `confirmed`, que e o campo que ESCONDE o eco — o usuario
    # ficaria sem nenhum sinal de que a resposta nao chegou.
    import json
    import time as _t
    q = PromptQueue("dupla")
    agora = _t.time()
    q.path.write_text(
        json.dumps({"id": "e1", "text": "Sim", "ts": agora - 100, "delivered": True, "desistiu": True}) + "\n" +
        json.dumps({"id": "e2", "text": "Sim", "ts": agora - 90, "delivered": True, "desistiu": True}) + "\n",
        encoding="utf-8")
    q.reconcile_delivered({"Sim"}, 0.0, agora)      # UMA linha "Sim" no transcript
    rows = q.load()
    assert sum(1 for r in rows if r.get("confirmed")) == 1     # so uma foi resgatada
    assert sum(1 for r in rows if r.get("desistiu")) == 1      # a outra segue marcada como perdida


def test_resgate_ignora_entrada_de_sessao_anterior():
    # Entrada de antes do /clear comparada contra o transcript de AGORA: texto curto e repetido
    # ("Sim", "1") casaria por coincidencia e daria por entregue o que nunca chegou.
    import json
    import time as _t
    q = PromptQueue("preclear")
    agora = _t.time()
    q.path.write_text(json.dumps({"id": "e1", "text": "Sim", "ts": agora - 5000,
                                  "delivered": True, "desistiu": True}) + "\n", encoding="utf-8")
    q.reconcile_delivered({"Sim"}, agora - 100, agora)   # min_ts = inicio da sessao atual
    r = q.load()[0]
    assert r.get("desistiu") is True and "confirmed" not in r


def test_reconcile_resgata_desistida_quando_o_eco_tem_sufixo(tmp_path):
    # Defeito (B) medido em 18/08/2026: a fila digitou "Vamos fazer ate as 23 com o Deepseek..."
    # e o transcript gravou a MESMA linha com "… eu tinha mandado isso" no fim. O casamento por
    # linha exata nunca casava -> a entrega desistia e a bolha ficava marcada "nao chegou" pra
    # sempre sobre uma msg que CHEGOU. O resgate tem de casar por PREFIXO (piso de comprimento).
    import json
    X = "Vamos fazer ate as 23 com o Deepseek dps agnt para e volta amanha"
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user", "content": X + "… eu tinha mandado isso"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(json.dumps({"id": "b1", "text": X, "ts": 1787075400.0,
                                  "delivered": True, "attempts": 2, "desistiu": True}) + "\n",
                      encoding="utf-8")
    q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787075500.0)
    row = q.load()[0]
    assert row["confirmed"] is True
    assert "desistiu" not in row          # a marca "nao chegou" saiu da bolha
    # e o merged_history nao mostra mais a bolha da fila (a real cobre)
    assert all(not e.id.startswith("queued-") for e in pqueue.merged_history("s", str(j)))


def test_reconcile_prefixo_curto_nao_resgata(tmp_path):
    # Piso do prefixo: "ok"/"sim" nao podem confirmar frase alheia que comeca igual — resposta
    # curta desistida continua desistida (honesta: nao ha prova de que chegou).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user", "content": "ok, vamos fazer"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text('{"id":"b2","text":"ok","ts":1787075400.0,"delivered":true,"attempts":2,"desistiu":true}\n',
                      encoding="utf-8")
    q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787075500.0)
    row = q.load()[0]
    assert row["desistiu"] is True and "confirmed" not in row


def test_reconcile_confirma_por_prefixo_sem_desistir(tmp_path):
    # Mesmo eco com sufixo, mas a entrada ainda NAO desistiu (primeira checagem): confirmar por
    # prefixo evita a redigitacao (o drain reenviaria o texto ja presente no prompt final).
    import json
    X = "Vamos fazer ate as 23 com o Deepseek dps agnt para e volta amanha"
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user", "content": X + "… eu tinha mandado isso"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(json.dumps({"id": "b3", "text": X, "ts": 1787075400.0, "delivered": True}) + "\n",
                      encoding="utf-8")
    req = q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787075500.0)
    assert req == []                            # nada re-enfileirado: nao redigita
    row = q.load()[0]
    assert row["confirmed"] is True


def test_committed_lines_NAO_contam_recado_preso_em_system(tmp_path):
    # Parecer G2 rev1, bloqueador 1: recado preso em entrega BLOQUEADA por hook tem
    # preventContinuation=true — o agente NUNCA o recebeu. committed_user_lines e o oraculo de
    # "aterrissou na sessao": conta-lo confirmaria a entrega e a bolha ficaria sem a marca
    # vermelha sobre uma mensagem que nao chegou (o proprio diagnostico da Task: "nesse caso a
    # marca esta CERTA").
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "system", "content":
                             "UserPromptSubmit operation blocked by hook:\n[x]\n\n"
                             "Original prompt: [de: exec2] RODADA 3 ENTREGUE — correcoes aplicadas."}) + "\n" +
                 json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user", "content": "uma msg normal que chegou"}}) + "\n",
                 encoding="utf-8")
    lines = pqueue.committed_user_lines(str(j))
    assert "[de: exec2] RODADA 3 ENTREGUE — correcoes aplicadas." not in lines
    assert "uma msg normal que chegou" in lines   # o oraculo segue vivo: so o system fica fora


def test_reconcile_desistida_mantem_marca_com_system_no_transcript(tmp_path):
    # Desfecho do bloqueador 1: entrada ja desistida + transcript que tem SO o system com o
    # recado (nunca virou user) → a entrada segue desistida, sem confirmed. O reconcile re-drena
    # ate max_attempts e desiste (limitado, pqueue.py:437) — a marca vermelha e a verdade.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "system", "content":
                             "UserPromptSubmit operation blocked by hook:\n[x]\n\n"
                             "Original prompt: [de: exec2] RODADA 3 ENTREGUE — correcoes aplicadas."}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.path.write_text(json.dumps({"id": "b1", "text": "[de: exec2] RODADA 3 ENTREGUE — correcoes aplicadas.",
                                  "ts": 1787075400.0, "delivered": True, "attempts": 2,
                                  "desistiu": True}) + "\n", encoding="utf-8")
    q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787075500.0)
    row = q.load()[0]
    assert row["desistiu"] is True
    assert "confirmed" not in row


def test_reconcile_prefixo_nao_rouba_linha_do_exato(tmp_path):
    # Parecer G2 rev1, bloqueador 2: X = "pode seguir" (perdida) / Y = "pode seguir com a Task 4
    # agora" (chegou). Sem as RESERVADAS, o prefixo de X consumia a linha exata de Y: X virava
    # entregue (falha escondida) e Y ganhava a marca "nao chegou" (o defeito da Task reaberto).
    # A linha que casa EXATO com Y pertence a Y — nenhum prefixo a leva.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user", "content": "pode seguir com a Task 4 agora"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    rows = [
        {"id": "X", "text": "pode seguir", "ts": 1787075400.0, "delivered": True,
         "attempts": 2, "desistiu": True},
        {"id": "Y", "text": "pode seguir com a Task 4 agora", "ts": 1787075500.0,
         "delivered": True, "attempts": 2},
    ]
    q.path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787100000.0)
    got = {r["id"]: r for r in q.load()}
    assert got["X"]["desistiu"] is True and "confirmed" not in got["X"]  # perdida, honesta
    assert got["Y"]["confirmed"] is True and "desistiu" not in got["Y"]  # chegou, sem marca


def test_reconcile_prefixo_nao_rouba_linha_do_mais_especifico(tmp_path):
    # Parecer G2 rev2, bloqueador 1: eco chega COM sufixo -> nenhuma das entradas casa exato,
    # reservadas fica vazia, e a entrada mais CURTA levava a linha da mais especifica:
    #   X = "Vamos fazer" (perdida) / Y = "Vamos fazer ate as 23 com o Deepseek" (chegou)
    #   transcript: "Vamos fazer ate as 23 com o Deepseek — eu tinha mandado isso"
    # A linha pertence a quem a reivindica de forma MAIS ESPECIFICA (dono = maior prefixo).
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "timestamp": "2026-08-18T01:04:51Z",
                             "message": {"role": "user",
                             "content": "Vamos fazer ate as 23 com o Deepseek — eu tinha mandado isso"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    rows = [
        {"id": "X", "text": "Vamos fazer", "ts": 1787075400.0, "delivered": True,
         "attempts": 2, "desistiu": True},
        {"id": "Y", "text": "Vamos fazer ate as 23 com o Deepseek", "ts": 1787075500.0,
         "delivered": True, "attempts": 2},
    ]
    q.path.write_text("".join(json.dumps(r) + "\n" for r in rows), encoding="utf-8")
    q.reconcile_delivered(pqueue.committed_user_lines(str(j)), min_ts=100.0, now=1787100000.0)
    got = {r["id"]: r for r in q.load()}
    assert got["X"]["desistiu"] is True and "confirmed" not in got["X"]  # perdida, honesta
    assert got["Y"]["confirmed"] is True and "desistiu" not in got["Y"]  # chegou, sem marca


def test_kickoff_de_bastao_nao_e_carimbado_como_sessao_anterior():
    # A outra rede de segurança que comia o kick-off: `ts < min_ts` marcava `confirmed` ("sessão
    # anterior: fora do escopo"), e confirmada nunca mais é reentregue nem aparece no histórico.
    # Com `pre_transcript` a entrada segue o caminho NORMAL — não achou no transcript, re-enfileira
    # pro drain (ou seja, a marca isenta do corte por idade, não do reconcile).
    q = PromptQueue("s")
    q.append("kick-off do bastão", delivered=True, ts=100.0, pre_transcript=True)
    q.reconcile_delivered(committed=set(), min_ts=500.0, now=200.0)
    r = PromptQueue("s").load()[0]
    assert "confirmed" not in r
    assert r["delivered"] is False and r["attempts"] == 1


def test_kickoff_de_bastao_aparece_no_historico_do_transcript_novo(tmp_path):
    # merged_history poda entrada anterior ao início da sessão (fantasma de pré-/clear). O kick-off
    # é anterior de propósito — sem a isenção a bolha dele nunca apareceria no chat da sessão nova.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "primeira linha"}}) + "\n",
                 encoding="utf-8")
    inicio = pqueue._transcript_start_ts(str(j))
    q = PromptQueue("s")
    q.append("velha de outra vida", delivered=False, ts=inicio - 3600)
    # 30s antes do transcript nascer = o intervalo REAL entre enfileirar o kick-off e a sessão
    # gravar a 1a linha.
    q.append("kick-off do bastão", delivered=False, ts=inicio - 30, pre_transcript=True)
    textos = [e.text for e in pqueue.merged_history("s", str(j))]
    assert "kick-off do bastão" in textos
    assert "velha de outra vida" not in textos


def test_entrada_sem_ts_herda_o_relogio_anterior_e_nao_some_do_historico(tmp_path):
    # Carry-forward, como o docstring de merged_history promete: `append` sempre carimba `ts`,
    # então isto é sidecar legado (ou editado à mão). Cortar por 0.0 dava DUAS idades à mesma
    # entrada dentro da mesma função — ordenada pelo relógio herdado e descartada pelo zero — e ela
    # sumia do histórico calada, que é o oposto do que a fila durável existe pra garantir.
    import json
    j = tmp_path / "t.jsonl"
    j.write_text(json.dumps({"type": "user", "uuid": "u0", "timestamp": "2026-01-01T00:00:00Z",
                             "message": {"role": "user", "content": "primeira linha"}}) + "\n",
                 encoding="utf-8")
    q = PromptQueue("s")
    q.append("sem relogio", delivered=False, ts=0.0)
    assert q.load()[0]["ts"] == 0.0             # o zero é o que o `or` do append deixa passar
    assert "sem relogio" in [e.text for e in pqueue.merged_history("s", str(j))]


def test_prune_before_nao_apaga_o_kickoff_de_bastao():
    q = PromptQueue("s")
    q.append("velha", delivered=False, ts=1000.0)
    q.append("kick-off", delivered=False, ts=1000.0 + 30, pre_transcript=True)
    assert q.prune_before(1000.0 + 60) == 1
    assert [e["text"] for e in PromptQueue("s").load()] == ["kick-off"]


def test_kickoff_de_bastao_expira_e_nao_ressuscita_numa_vida_posterior():
    # A isenção do bastão tem PRAZO. Sem ele, um kick-off nunca entregue (sessão morta antes de a
    # TUI aceitar texto) sobreviveria a todo corte futuro e seria digitado na PRÓXIMA sessão de
    # mesmo nome — a dívida da vida anterior que o corte por idade existe pra matar. E o
    # cheap-check do drain ficaria quente naquele arquivo pra sempre.
    q = PromptQueue("s")
    velho = 1000.0
    q.append("kick-off de uma vida anterior", delivered=False, ts=velho, pre_transcript=True)
    assert q.prune_before(velho + pqueue._JANELA_BASTAO + 1) == 1
    assert PromptQueue("s").load() == []


def test_confirm_delivered_carimba_so_as_entregues_nao_confirmadas():
    # O steer do Kimi baixa a fila INTERNA da TUI — que é exatamente o conjunto delivered e ainda
    # não confirmado. Confirmada já, não-entregue e desistida não podem mudar.
    q = PromptQueue("s")
    q.append("ja confirmada", delivered=True)
    q.append("na fila da TUI", delivered=True)
    q.append("nem digitada", delivered=False)
    rows = q.load()
    rows[0]["confirmed"] = True
    q._write_atomic(rows)

    assert q.confirm_delivered() == 1
    final = PromptQueue("s").load()
    assert final[0].get("confirmed") is True
    assert final[1].get("confirmed") is True
    assert "confirmed" not in final[2]
    # Segunda passada não carimba nada (idempotente — clique duplo no chip).
    assert q.confirm_delivered() == 0
