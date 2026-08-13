"""Cobertura do sidecar de fila durável (pqueue): append/load, clear, e rename (move preservando
entradas). Isola o queue dir apontando settings.projects_dir pra um tmp."""
import os

import pytest

from app import pqueue
from app.pqueue import PromptQueue


@pytest.fixture(autouse=True)
def _tmp_queue_dir(tmp_path, monkeypatch):
    # _queue_dir() = settings.projects_dir.parent / ".claude-pocket-queue" -> redireciona pro tmp.
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


def test_prune_old_apaga_so_o_que_passou_do_prazo(tmp_path):
    """Limpeza por idade: anexo velho sai, recente fica, e days<=0 desliga a varredura."""
    import time as _t
    from app.uploads import prune_old, UPLOAD_SUBDIR

    base = tmp_path / UPLOAD_SUBDIR
    base.mkdir()
    velho, novo = base / "velho.png", base / "novo.png"
    velho.write_bytes(b"x")
    novo.write_bytes(b"x")
    antigo = _t.time() - 40 * 86400
    os.utime(velho, (antigo, antigo))

    assert prune_old(str(tmp_path), 0) == 0        # desligado -> não toca em nada
    assert velho.exists()

    assert prune_old(str(tmp_path), 30) == 1
    assert not velho.exists()
    assert novo.exists()                            # dentro do prazo, fica

    # Pasta inexistente não é erro (sessão que nunca recebeu anexo).
    assert prune_old(str(tmp_path / "nada"), 30) == 0


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


def test_confirm_adia_enquanto_trabalha(tmp_path, monkeypatch):
    # Guard que ja existia: mid-turn nao se mexe na fila (nem confirma, nem redigita).
    import time as _t
    chamou, row = _cenario_engolida(tmp_path, monkeypatch, ("working", _t.time()))
    assert chamou == []
    assert "confirmed" not in row and not row.get("attempts")


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
