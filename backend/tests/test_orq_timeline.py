import json
import os
import shutil
from datetime import datetime, timedelta
from pathlib import Path

import pytest

from orq_contract_client import timeline as ot, adapter, call
from app import pqueue

FIX = Path(__file__).parent / "fixtures" / "orq"
PARECER = "/home/jefferson/.hangar/orq/2026-09-29-cad3e6fe/pareceres/task-4-r1-revisor.md"


@pytest.fixture
def real(tmp_path):
    d = tmp_path / "2026-09-29-cad3e6fe"          # o nome da pasta dá o nome da linha do tempo
    shutil.copytree(FIX / "2026-09-29-cad3e6fe", d)
    cfg = json.loads((d / "orq.json").read_text())
    (d / "orq.json").write_text(json.dumps({**cfg, "plan": str(d / "plan.orq.md")}))
    return d


@pytest.fixture
def synth(tmp_path):
    d = tmp_path / "synthetic"
    shutil.copytree(FIX / "synthetic", d)
    return d


def lines(d):
    return [json.loads(l) for l in ot.timeline_path(d).read_text().splitlines() if l.strip()]


def entries(d):
    run = ot.run_files(d)
    return [ot.entry(o, run) for o in lines(d)]


def test_real_fixture_has_the_cut_run(real):
    assert len(lines(real)) == 13


def test_opened_line(real):
    e = entries(real)[0]
    assert e["kind"] == "advance" and e["task"] == 1
    assert e["line"] == {"code": "opened", "sessions": [
        {"name": "w-t1", "provider": "claude", "model": "opus[1m]"},
        {"name": "w-review1", "provider": "claude", "model": "opus[1m]"}]}
    assert e["origin"] is None and e["decided_by"] is None


def test_integrated_line_with_and_without_merge(real):
    assert entries(real)[2]["line"] == {"code": "integrated", "merge": True}
    obj = {**lines(real)[2], "text": "T1 fechada → integração verde"}
    assert ot.entry(obj, None)["line"] == {"code": "integrated", "merge": False}


def test_vigia_alarm(real):
    e = entries(real)[1]
    assert e["kind"] == "woke" and e["origin"] == "notify" and e["alarm"] is True
    assert e["sender"] == "vigia" and e["task"] is None
    assert e["body"].startswith("ARMED over: w-arbiter")
    assert e["decided_by"] == {"source": "alarm", "rule": None, "jev": None, "regex": None, "regex_agreed": None}


def test_decision_without_parecer(real):
    e = entries(real)[7]
    assert e["mark"] == "decisao" and e["task"] == 4
    assert e["question"] == "Autoriza detalhe recebido como dado já seguro, ou define redator mínimo/pendência?"
    assert e["parecer"] is None
    assert e["body"].startswith("`StateEvent.problema_detalhe` é documentado como detalhe cru do processo (`core/types.ts:214`)")
    assert "PendingPlan e AssistantBubble podem avançar enquanto isso." in e["body"]
    assert e["decided_by"]["source"] == "rule" and e["decided_by"]["rule"] == "mark"


def test_decision_with_parecer(real):
    e = entries(real)[9]
    assert e["parecer"] == PARECER
    assert e["question"] == "Incluir `MessageList.tsx:104` em T4 (passar `ev.text` cru) ou deixar para T11?"
    assert "(NOTED 1)" not in e["body"] and "Parecer:" not in e["body"]


def test_rejected_round(real):
    e = entries(real)[10]
    assert e["rejected_round"] == 1 and e["task"] == 4
    assert e["question"] is None      # o `??` de `proposed ?? assistantText` não é pergunta
    assert e["parecer"] == PARECER
    assert "`planDisplayText(text)`" in e["body"] and "`executor.md`" in e["body"]


def test_dropped_by_jev(synth):
    e = entries(synth)[0]
    assert e["kind"] == "dropped" and e["sender"] == "w-t4" and e["origin"] == "notify"
    assert e["body"] == "Rodada 2 entregue ao revisor, sigo aguardando o veredito."
    d = e["decided_by"]
    assert d["source"] == "jev" and d["jev"]["p"] == 0.97
    assert d["jev"]["probs"] == {"nothing": 0.97, "act": 0.03}
    assert d["regex"] == {"verdict": "drop", "category": "entrega"} and d["regex_agreed"] is True


def test_dropped_where_regex_disagrees(synth):
    d = entries(synth)[1]["decided_by"]
    assert d["source"] == "jev" and d["regex"]["verdict"] == "wake" and d["regex_agreed"] is False


def test_would_drop_by_regex_without_jev(synth):
    e = entries(synth)[2]
    d = e["decided_by"]
    assert e["kind"] == "would_drop" and d["source"] == "regex"
    assert d["regex"] == {"verdict": "drop", "category": "janela"}
    assert d["jev"] is None and d["regex_agreed"] is None
    assert e["sender"] is None and e["origin"] == "notify"


def test_woke_with_jev_act(synth):
    e = entries(synth)[3]
    d = e["decided_by"]
    assert d["source"] == "jev" and d["jev"]["choice"] == "act" and d["jev"]["probs"]["act"] == 0.94
    assert e["task"] == 5 and e["question"] == "preciso da tela do celular, posso usar?"
    assert d["regex_agreed"] is True


def test_jev_error_falls_back_to_regex(synth):
    d = entries(synth)[4]["decided_by"]
    assert d["source"] == "regex" and d["jev"]["error"].startswith("URLError")
    assert d["regex_agreed"] is None


def test_old_record_without_probs_or_sender(synth):
    e = entries(synth)[5]
    d = e["decided_by"]
    assert e["origin"] == "notify" and e["sender"] is None and d["source"] == "jev"
    assert d["jev"]["probs"] == {} and d["jev"]["held"] == ["problem"]


def test_notify_that_failed(synth):
    e = entries(synth)[6]
    assert e["kind"] == "failed" and e["origin"] == "notify"
    assert e["error"] == "hangar-send w-arbiter failed (rc=1): boom"
    assert e["mark"] == "decisao" and e["task"] == 5 and e["decided_by"] is None


def test_orchestrator_failure(synth):
    e = entries(synth)[7]
    assert e["origin"] == "orchestrator" and e["task"] == 3
    assert e["body"] == "falhou ao integrar a T3: merge abortado" and e["decided_by"] is None


def test_orchestrator_wake(synth):
    e = entries(synth)[8]
    assert e["origin"] == "orchestrator"
    assert e["decided_by"]["source"] == "rule" and e["decided_by"]["rule"] == "orchestrator"


def test_notice_has_no_line(synth):
    e = entries(synth)[9]
    assert e["kind"] == "notice" and e["task"] == 5 and e["line"] is None


def test_delivered_and_red_back(synth):
    es = entries(synth)
    assert es[10]["line"] == {"code": "delivered", "round": 1, "commit": "7858997"}
    assert es[11]["line"] == {"code": "red_back", "executor": "w-t3"}


def test_red_retry_line():
    o = {"ts": "2026-09-29T22:00:00-03:00", "kind": "advance", "task": 3,
         "text": "T3: integração vermelha, rodando de novo uma vez"}
    assert ot.entry(o, None)["line"] == {"code": "red_retry"}


def test_jev_record_outside_the_window_is_not_matched(synth):
    obj = lines(synth)[0]
    run = ot.run_files(synth)
    rows = [json.loads(l) for l in (synth / "jev-shadow.jsonl").read_text().splitlines()]
    when = ot.orq_conductor._when(rows[0]["ts"]) - timedelta(seconds=60)
    rows[0]["ts"] = when.isoformat()
    (synth / "jev-shadow.jsonl").write_text("\n".join(json.dumps(r) for r in rows) + "\n")
    e = ot.entry(obj, run)
    assert e["decided_by"]["jev"] is None and e["decided_by"]["source"] == "jev"


def test_entry_without_run_never_raises(synth):
    for o in lines(synth):
        e = ot.entry(o, None)
        if e["decided_by"] and e["decided_by"]["source"] != "rule":
            assert e["decided_by"]["jev"] is None and e["decided_by"]["regex"] is None


def test_empty_text_is_none():
    assert ot.entry({"kind": "woke", "text": "  "}, None) is None
    assert ot.entry({"kind": "woke"}, None) is None


def test_unknown_kind_is_a_notice():
    assert ot.entry({"kind": "??", "text": "algo"}, None)["kind"] == "notice"


def test_jev_index_is_read_once_until_the_file_changes(synth, monkeypatch):
    rows=call("snapshot.cache",evict_updated=True,steps=[{"key":"run","signature":[1,2000],"value":{"index":1}},{"key":"run","signature":[1,2000]},{"key":"run","signature":[2,2001],"value":{"index":2}}])
    assert rows[0]["value"] == rows[1]["value"] == {"index":1}
    assert rows[0]["count"] == rows[1]["count"] == 1
    assert rows[2]["count"] == 2 and rows[2]["value"] == {"index":2}


def test_bad_jev_lines_are_skipped(synth):
    f = synth / "jev-shadow.jsonl"
    f.write_text("não é json\n[1]\n" + '{"ts": "x", "text": "a"}\n' + f.read_text())
    assert entries(synth)[0]["decided_by"]["jev"]["p"] == 0.97


def test_parse_obj_keeps_text_and_adds_orq(synth):
    obj = lines(synth)[3]
    [ev] = adapter.parse_obj(obj, synth)
    assert ev.kind == "notice" and ev.text == obj["text"] and ev.orq["kind"] == "woke"
    [plain] = adapter.parse_obj(obj)
    assert plain.id == ev.id and plain.orq["decided_by"]["jev"] is None


def test_history_carries_orq(synth):
    evs=[__import__("types").SimpleNamespace(**row) for row in call("timeline.history",path=ot.timeline_path(synth))]
    assert len(evs) == 12 and all(e.orq for e in evs)
    assert evs[3].orq["decided_by"]["jev"]["choice"] == "act"


def test_tailer_parser_uses_the_run_dir(synth):
    parse = adapter.line_parser(synth)
    [ev] = parse(json.dumps(lines(synth)[3], ensure_ascii=False))
    assert ev.orq["decided_by"]["jev"]["choice"] == "act"


def test_entry_failure_still_yields_a_plain_notice(synth, monkeypatch):
    obj=lines(synth)[3]
    [row]=call("timeline.parse-failed",obj=obj)
    ev=__import__("types").SimpleNamespace(**row)
    assert ev.kind == "notice" and ev.text == obj["text"] and ev.orq is None and ev.id.startswith("orq:")


def test_run_files_reuses_one_and_caps_the_cache(tmp_path):
    rows=call("snapshot.cache",evict_updated=True,steps=[{"key":"run","signature":1,"value":"same"},{"key":"run","signature":1}]+[{"key":f"r{i}","signature":1,"value":i} for i in range(20)])
    assert rows[1]["value"] == rows[0]["value"] and rows[1]["count"] == 1
    assert rows[-1]["len"] <= 16


def test_tasks_da_execucao_real(real):
    t = ot.panel(real)["tasks"]
    assert (t["integrated"], t["total"], t["total_known"]) == (4, 44, True)
    by = {r["n"]: r for r in t["rows"]}
    assert [by[n]["state"] for n in (1, 2, 3, 4)] == ["integrated"] * 4
    assert (by[5]["state"], by[5]["round"]) == ("in_review", 1)
    assert by[6]["state"] == "queued" and by[6]["title"] == "Silêncio SSE e recusa HTTP separados"


def _cut(d, n):
    ev = (d / "eventos.jsonl").read_text().splitlines()
    (d / "eventos.jsonl").write_text("\n".join(ev[:n]) + "\n")


def test_estados_intermediarios(real):
    _cut(real, 20)                                   # até o veredito que reprova a T4 R1 (22:12:21)
    by = {r["n"]: r for r in ot.panel(real)["tasks"]["rows"]}
    assert (by[4]["state"], by[4]["round"]) == ("rejected", 1)
    assert by[2]["state"] == "integrated" and by[5]["state"] == "queued"
    _cut(real, 18)                                   # veredito aprova T2 R2, antes da integração
    assert {r["n"]: r for r in ot.panel(real)["tasks"]["rows"]}[2]["state"] == "approved"


def test_time_da_execucao_real(real):
    team = ot.panel(real)["team"]
    assert team[0] == {"name": "w-arbiter", "role": "arbiter", "task": None, "last": None, "current": True}
    names = [m["name"] for m in team]
    assert len(names) == 11 and names[1:3] == ["hangar-mobile-t5", "hangar-mobile-review5"]
    t5 = team[1]
    assert (t5["role"], t5["task"], t5["last"]["code"], t5["last"]["round"]) == ("executor", 5, "delivered", 1)
    rev4 = next(m for m in team if m["name"] == "w-review4")
    assert (rev4["last"]["code"], rev4["last"]["round"]) == ("approved", 2)


def test_time_depois_de_trocas(real):
    with (real / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "sessao_trocada", "de": "hangar-mobile-t5",
                            "para": "hangar-mobile-t5b", "papel": "executor", "task": 5, "motivo": "x"}) + "\n")
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "sessao_trocada", "de": "w-arbiter",
                            "para": "w-arbiter-2", "papel": "arbitro", "motivo": "y"}) + "\n")
    team = ot.panel(real)["team"]
    assert team[0]["name"] == "w-arbiter-2" and team[0]["current"] is True
    assert team[1] == {**team[1], "name": "w-arbiter", "role": "arbiter", "current": False}
    t5b = next(m for m in team if m["name"] == "hangar-mobile-t5b")
    assert (t5b["role"], t5b["task"], t5b["last"]["code"]) == ("executor", 5, "swapped_in")
    assert next(m for m in team if m["name"] == "hangar-mobile-t5")["current"] is False


def test_revisor_subagente_ou_frase_fica_de_fora(real):
    with (real / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "task_inicio", "task": 6, "titulo": "t",
                            "executor": "x6", "par": "usuario aprova a folha"}) + "\n")
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "task_inicio", "task": 7, "titulo": "t",
                            "executor": "x7", "par": "subagente"}) + "\n")
    names = [m["name"] for m in ot.panel(real)["team"]]
    assert "x6" in names and "x7" in names and "usuario aprova a folha" not in names and "subagente" not in names


def test_decisoes_abertas_e_fechadas(real):
    assert ot.panel(real)["decisions"] == []          # a T4 entregou a R2 depois das três
    _cut(real, 20)
    d = ot.panel(real)["decisions"]
    assert [x["ts"] for x in d] == ["2026-09-29T22:12:48-03:00", "2026-09-29T22:12:22-03:00"]
    assert d[1]["question"] == "Incluir `MessageList.tsx:104` em T4 (passar `ev.text` cru) ou deixar para T11?"
    assert d[0]["question"].startswith("receita exige")   # sem "?": a primeira frase, até 160 caracteres
    assert d[0]["parecer"].endswith("pareceres/task-4-r1-revisor.md")
    assert len(d[0]["question"]) <= 160 and all(x["event_id"].startswith("orq:") for x in d)


def test_automacao_da_execucao_real(real):
    a = ot.panel(real)["automation"]
    assert a["mode"] == {"jev": "on", "regex": "shadow"}
    assert a["woke"] == {"total": 4, "decisions": 3, "alarms": 1, "messages": 0}
    assert a["alone"] == {"total": 9, "opened": 5, "integrated": 4, "dropped": 0}
    assert a["dropped_by_jev"] == 0
    assert a["advanced"] == {"would_drop": 0, "disagree": 0, "judged": 0, "min_confidence": None, "by_rule": 4}


def test_modo_sem_chave_jev_e_sombra(real):
    cfg = json.loads((real / "orq.json").read_text())
    cfg.pop("jev")
    (real / "orq.json").write_text(json.dumps(cfg))
    assert ot.panel(real)["automation"]["mode"]["jev"] == "shadow"   # o mesmo padrão do orq.py


def test_automacao_sintetica(synth):
    a = ot.panel(synth)["automation"]
    assert a["alone"]["dropped"] == 2 and a["dropped_by_jev"] == 2
    assert a["advanced"]["would_drop"] == 1
    assert (a["advanced"]["disagree"], a["advanced"]["judged"]) == (1, 4)
    assert a["advanced"]["min_confidence"]["p"] == 0.62


def test_integracao_da_execucao_real(real):
    i = ot.panel(real)["integration"]
    assert i["branch"] == "mobile-deliveries-orq"
    assert i["last"] == {"task": 4, "commit": "538ac4d", "ts": "2026-09-29T22:19:00-03:00"}
    assert i["outcome"] == "green" and i["delivery_checks"] == {"ok": 5, "total": 5, "failing": []}


def test_integracao_vermelha_e_falha_do_advance(real):
    with (real / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "advance_falhou", "passo": "integrate",
                            "task": 4, "motivo": "boom"}) + "\n")
    with (real / "closed.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:25:58-03:00", "task": 4, "hash": "a" * 40}) + "\n")
    p = ot.panel(real)
    assert p["integration"]["outcome"] == "failed"
    assert {r["n"]: r for r in p["tasks"]["rows"]}[4]["state"] == "integration_red"


def test_arquivos_faltando_e_ilegiveis(tmp_path, real):
    only = tmp_path / "so-timeline"
    only.mkdir()
    shutil.copy(real / "timeline-2026-09-29-cad3e6fe.jsonl", only / "timeline-so-timeline.jsonl")
    p = ot.panel(only)
    assert p["tasks"]["rows"] == [] and p["team"] == [] and p["integration"]["last"] is None
    assert [e["file"] for e in p["errors"]] == ["orq.json", "consumption"] and p["empty"] is False


@pytest.mark.skipif(os.geteuid() == 0, reason="root lê arquivo sem permissão")
def test_arquivo_ilegivel_vira_erro(real):
    (real / "eventos.jsonl").chmod(0)
    try:
        assert any(e["file"] == "eventos.jsonl" for e in ot.panel(real)["errors"])
    finally:
        (real / "eventos.jsonl").chmod(0o644)


def test_plano_ilegivel(real):
    (real / "plan.orq.md").unlink()
    p = ot.panel(real)
    assert p["tasks"]["total_known"] is False and p["tasks"]["total"] == 5
    assert [e["file"] for e in p["errors"]] == ["plan"]


def test_execucao_recem_criada_e_vazia(tmp_path):
    d = tmp_path / "nova"
    d.mkdir()
    (d / "orq.json").write_text('{"arbiter": "arb", "auto": true, "jev": "on", "regex": "shadow"}')
    assert ot.panel(d)["empty"] is True


def test_linha_do_tempo_grande_usa_o_cache(real, monkeypatch):
    tl=ot.timeline_path(real)
    tl.write_text(tl.read_text()*400)
    first=ot.panel(real)
    rows=call("snapshot.cache",steps=[{"key":str(real),"signature":[1,tl.stat().st_size],"value":first},{"key":str(real),"signature":[1,tl.stat().st_size]}])
    assert rows[1]["value"] == first and rows[1]["count"] == 1


def test_evento_malformado_nao_vira_500(real):
    with (real / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:26:00-03:00", "tipo": "task_inicio", "executor": "x"}) + "\n")
        f.write(json.dumps({"ts": "2026-09-29T22:26:01-03:00", "tipo": "veredito", "task": 5, "sessao": "y",
                            "resultado": ["aprova"]}) + "\n")
    p = ot.panel(real)
    assert p["errors"] and all(e["file"] for e in p["errors"])
    assert p["tasks"]["rows"] and p["automation"]["mode"]["jev"] == "on"   # o resto do retrato sai igual


def test_bloco_que_estoura_vira_erro_e_o_resto_sai(real, monkeypatch):
    p=call("panel.failed",directory=real,home=Path.home(),block="decisions")
    assert p["decisions"] == [] and [e["file"] for e in p["errors"]] == ["decisions"]
    assert len(p["team"]) == 11


def test_trocados_ficam_no_bloco_da_task(real):
    with (real / "eventos.jsonl").open("a") as f:
        f.write(json.dumps({"ts": "2026-09-29T22:25:59-03:00", "tipo": "sessao_trocada", "de": "hangar-mobile-t5",
                            "para": "hangar-mobile-t5b", "papel": "executor", "task": 5, "motivo": "x"}) + "\n")
    team = ot.panel(real)["team"]
    names = [m["name"] for m in team]
    i = names.index("hangar-mobile-t5")
    assert names[i - 2:i] == ["hangar-mobile-t5b", "hangar-mobile-review5"] and team[i]["task"] == 5
    assert team[i + 1]["task"] == 4                  # o bloco da Task 4 vem depois, não o fim da lista


def test_jsonl_com_byte_cortado_nao_zera_o_painel(real):
    p = real / "eventos.jsonl"
    p.write_bytes(p.read_bytes() + '{"tipo": "x", "motivo": "é'.encode()[:-1])   # meio caractere multibyte
    out = ot.panel(real)
    assert out["tasks"]["rows"] and not any(e["file"] == "eventos.jsonl" for e in out["errors"])


def _row(source, model, i, o, cw, cr, ts="2026-09-30T01:00:00+00:00"):
    from app.costs_sources import UsageRow
    return UsageRow(ts=datetime.fromisoformat(ts), source=source, provider="", model=model, project="/r",
                    session_id="s", input=i, output=o, cache_write=cw, cache_read=cr)


LIVE = lambda: [type("S", (), {"name": "hangar-mobile-t5", "provider": "claude", "jsonl": "/p/t5.jsonl"})()]


def _native_consumption(directory, rows, sources):
    parts=call("panel.parts",directory=directory,home=Path.home())
    names=[r["name"] for r in parts["out"]["team"]]
    return call("consumption.compute",directory=directory,names=names,aux=parts["aux"],live=[],sources=sources,rows=rows)["value"]


def _sources(real, tmp_path):
    rollout = tmp_path / "rollout-2026-09-29T21-46-41-01a0efc7-25ef-7ee0-b078-1d31dc2c7c7d.jsonl"
    rollout.write_text("")
    (real / "sessions.jsonl").write_text(json.dumps({"ts": "…", "name": "w-arbiter", "role": "arbitro",
        "task": None, "provider": "codex", "session_id": None, "config_dir": None,
        "thread_id": "01a0efc7-25ef-7ee0-b078-1d31dc2c7c7d", "codex_home": str(tmp_path)}) + "\n")
    return str(rollout)


def test_consumo_por_provider_e_modelo(real, tmp_path, monkeypatch):
    rollout=_sources(real,tmp_path)
    fake={rollout:[_row("codex","gpt-6.1-sol",100,10,0,1000),_row("codex","gpt-6.1-sol",999,0,0,0,ts="2026-09-29T20:00:00+00:00")],"/p/t5.jsonl":[_row("claude","claude-opus-5",50,5,20,500),_row("claude","claude-sonnet-5",1,1,0,10)]}
    before=sorted(p.name for p in real.iterdir())
    c=_native_consumption(real,fake,{"w-arbiter":{rollout:"codex"},"hangar-mobile-t5":{"/p/t5.jsonl":"claude"}})
    assert sorted(p.name for p in real.iterdir()) == before
    assert c["since"] == "2026-09-29T21:53:31-03:00"
    assert (c["sessions"]["team"],c["sessions"]["measured"]) == (11,2)
    assert "w-t1" in c["sessions"]["missing"]
    assert c["totals"]["new"] == 100+10+50+5+20+1+1 and c["totals"]["cache_read"] == 1510
    claude=next(p for p in c["providers"] if p["provider"] == "claude")
    assert [m["model"] for m in claude["models"]] == ["claude-opus-5","claude-sonnet-5"] and claude["sessions"] == 1


def test_medicao_serve_de_fonte_para_execucao_antiga(real, monkeypatch):
    transcript=real.parent/"rollout.jsonl"
    transcript.write_text("")
    (real/"medicao").mkdir()
    (real/"medicao/w-arbiter-inicio.json").write_text(json.dumps({"session":"w-arbiter","provider":"codex","model_observed":"gpt-6.1-sol","source":{"path":str(transcript)}}))
    names=[r["name"] for r in ot.panel(real)["team"]]
    sources=call("consumption.paths",directory=real,home=Path.home(),names=names,live=[])
    assert str(transcript.resolve()) in sources["w-arbiter"]
    c=_native_consumption(real,{str(transcript.resolve()):[_row("codex","gpt-6.1-sol",1,1,0,0)]},sources)
    assert c["sessions"]["measured"] == 1


def test_consumo_e_refeito_so_depois_do_ttl(real, monkeypatch):
    rows=call("consumption.cache",steps=[{"result":{},"now":0},{"result":{},"now":1},{"result":{},"now":1,"ttl":0}])
    assert rows[0]["count"] == rows[1]["count"] == 1
    assert rows[2]["count"] == 2


def test_modelo_sem_preco_marca_o_total_como_parcial(real, monkeypatch):
    c=_native_consumption(real,{"/p/t5.jsonl":[_row("claude","modelo-inventado",1,1,0,0)]},{"hangar-mobile-t5":{"/p/t5.jsonl":"claude"}})
    assert c["missing_prices"] == ["modelo-inventado"] and c["totals"]["usd_partial"] is True


def test_falha_no_consumo_vira_erro_e_o_resto_sai(real, monkeypatch):
    rows=call("consumption.cache",steps=[{"compute_error":"falha da fonte"},{}])
    assert rows[1]["value"] is None and rows[1]["error"] == "falha da fonte"
    assert len(ot.panel(real)["team"]) == 11


def test_ultima_linha_do_nome_vence_em_sessions_jsonl(real, tmp_path, monkeypatch):
    _sources(real,tmp_path)
    old=json.loads((real/"sessions.jsonl").read_text())
    new={**old,"thread_id":"bbbb"}
    (real/"sessions.jsonl").write_text(json.dumps(old)+"\n"+json.dumps(new)+"\n")
    (tmp_path/"sessions").mkdir()
    new_path=tmp_path/"sessions/rollout-2026-bbbb.jsonl"
    new_path.write_text("")
    old_path=tmp_path/"sessions/rollout-2026-01a0efc7-25ef-7ee0-b078-1d31dc2c7c7d.jsonl"
    old_path.write_text("")
    paths=call("consumption.paths",directory=real,home=Path.home(),names=["w-arbiter"],live=[])
    assert list(paths["w-arbiter"]) == [str(new_path.resolve())]


def test_primeiro_pedido_volta_na_hora_com_none_e_o_seguinte_traz_o_valor(real, monkeypatch):
    rows=call("consumption.cache",steps=[{"result":{"computed_at":"x"},"hold":True},{"hold":True},{"finish":True},{}])
    assert rows[0]["value"] is None and rows[0]["running"]
    assert rows[1]["value"] is None and rows[1]["queued"] == 1
    assert rows[2]["count"] == 1 and not rows[2]["running"]
    assert rows[3]["value"] == {"computed_at":"x"}


def test_consumo_vencido_devolve_o_ultimo_valor_enquanto_recalcula(real, monkeypatch):
    rows=call("consumption.cache",steps=[{"result":{"computed_at":"old"}}, {"now":61,"result":{"computed_at":"novo"},"hold":True}, {"now":62,"hold":True},{"finish":True},{"now":63}])
    assert rows[1]["value"] == rows[2]["value"] == {"computed_at":"old"}
    assert rows[2]["queued"] == 1
    assert rows[3]["count"] == 2 and not rows[3]["running"]
    assert rows[4]["value"] == {"computed_at":"novo"}


def test_time_que_falhou_nao_gera_consumo_zerado_nem_cache(real, monkeypatch):
    p=call("panel.failed",directory=real,home=Path.home(),block="team")
    assert p["consumption"] is None and [e["file"] for e in p["errors"]] == ["team","consumption"]
    assert p["team"] == []


def test_transcript_indisponivel_fica_em_missing_e_tenta_de_novo_logo(real, monkeypatch):
    value=_native_consumption(real,{"/p/t5.jsonl":None},{"hangar-mobile-t5":{"/p/t5.jsonl":"claude"}})
    rows=call("consumption.cache",steps=[{"result":value,"unavailable":True},{"now":4},{"now":6,"result":value,"unavailable":True}])
    assert rows[1]["value"]["sessions"]["measured"] == 0
    assert "hangar-mobile-t5" in rows[1]["value"]["sessions"]["missing"]
    assert rows[1]["count"] == 1
    assert rows[2]["count"] == 2


@pytest.fixture(params=["UTC", "America/Sao_Paulo"])
def machine_tz(request, monkeypatch):
    monkeypatch.setenv("TZ", request.param)
    yield request.param


def test_claude_do_dia_do_inicio_entra_inteiro_mesmo_antes_do_inicio(real, monkeypatch, machine_tz):
    fake={"/p/t5.jsonl":[_row("claude","claude-opus-5-5",50,5,0,0,ts="2026-09-29T20:00:00-03:00"),_row("claude","claude-opus-5-5",999,0,0,0,ts="2026-09-28T23:00:00-03:00")]}
    c=_native_consumption(real,fake,{"hangar-mobile-t5":{"/p/t5.jsonl":"claude"}})
    assert c["totals"]["new"] == 55


def test_thread_que_nao_sobe_libera_a_trava_e_o_proximo_pedido_calcula(real, monkeypatch):
    rows=call("consumption.cache",steps=[{"fail_spawn":True},{"now":6,"result":{"computed_at":"x"}},{"now":7}])
    assert rows[0]["value"] is None and "no threads" in rows[0]["error"]
    assert not rows[0]["running"]
    assert rows[2]["value"] == {"computed_at":"x"} and rows[2]["count"] == 1


def test_codex_sem_leitura_e_provider_desconhecido_nao_contam_como_zero(monkeypatch, tmp_path):
    path=tmp_path/"r.jsonl"
    assert ot._rows_for("codex",str(path)) is None
    assert ot._rows_for("ninguem",str(tmp_path/"x.jsonl")) is None
    path.write_text("")
    assert ot._rows_for("codex",str(path)) == []


def test_elapsed_time_includes_reviews_and_freezes_at_integration():
    evs = [
        {"tipo": "execucao_inicio", "ts": "2026-09-30T10:00:00+00:00"},
        {"tipo": "task_inicio", "task": 1, "ts": "2026-09-30T10:05:00+00:00"},
        {"tipo": "entrega", "task": 1, "ts": "2026-09-30T10:10:00+00:00"},
        {"tipo": "task_inicio", "task": 2, "ts": "2026-09-30T10:05:00+00:00"},
        {"tipo": "integrada", "task": 1, "ts": "2026-09-30T10:25:00+00:00"},
    ]
    out = {"tasks": {"rows": [{"n": 1}, {"n": 2}, {"n": 3}]}}
    aux = {"timing": ot._timing_bounds(evs)}
    now = datetime.fromisoformat("2026-09-30T10:30:00+00:00")
    first = ot._with_timing(out, aux, now)
    later = ot._with_timing(out, aux, now + timedelta(minutes=5))
    assert first["timing"]["elapsed_seconds"] == 1800
    assert [r["timing"]["elapsed_seconds"] for r in first["tasks"]["rows"]] == [1200, 1500, None]
    assert [r["timing"]["elapsed_seconds"] for r in later["tasks"]["rows"]] == [1200, 1800, None]
    assert out == {"tasks": {"rows": [{"n": 1}, {"n": 2}, {"n": 3}]}}
    evs.append({"tipo": "execucao_fim", "ts": "2026-09-30T10:30:00+00:00"})
    ended = ot._with_timing(out, {"timing": ot._timing_bounds(evs)}, now + timedelta(hours=3))
    assert ended["timing"]["elapsed_seconds"] == 1800
    assert ended["tasks"]["rows"][1]["timing"]["elapsed_seconds"] == 1500
    evs.append({"tipo": "entrega", "task": 1, "rodada": 2, "ts": "2026-09-30T10:32:00+00:00"})
    reopened = ot._with_timing(out, {"timing": ot._timing_bounds(evs)}, now + timedelta(minutes=5))
    assert reopened["timing"]["finished_at"] is None
    assert reopened["tasks"]["rows"][0]["timing"]["elapsed_seconds"] == 1800
    assert ot._task_rows(evs, [], {1})[0]["state"] == "in_review"
    evs.append({"tipo": "integrada", "task": 1, "ts": "2026-09-30T10:40:00+00:00"})
    closed = ot._with_timing(out, {"timing": ot._timing_bounds(evs)}, now + timedelta(hours=3))
    assert closed["tasks"]["rows"][0]["timing"]["elapsed_seconds"] == 2100


@pytest.mark.parametrize("start,finish", [
    (None, None), ("invalid", None),
    ("2026-09-30T11:00:00+00:00", "2026-09-30T10:00:00+00:00"),
    ("2026-09-30T09:00:00+00:00", "invalid"),
])
def test_elapsed_time_does_not_invent_missing_or_invalid_dates(start, finish):
    timing = {"started_at": start, "finished_at": finish, "tasks": {}}
    out = ot._with_timing({"tasks": {"rows": []}}, {"timing": timing},
                          datetime.fromisoformat("2026-09-30T10:00:00+00:00"))
    assert out["timing"]["elapsed_seconds"] is None


def test_history_name_comes_from_plan_and_survives_missing_plan(tmp_path):
    plan=tmp_path/"2026-09-30-mobile-deliveries.md"
    content="# Entregas do aplicativo móvel\n\n## Tasks\n\n| # | What it is |\n|---|---|\n| 1 | Enviar mensagem |\n"
    plan.write_text(content)
    (tmp_path/"orq.json").write_text(json.dumps({"plan":str(plan),"repo":"/fixture/project"}))
    # O motor externo grava o snapshot; o leitor canônico precisa sobreviver à remoção do plano.
    (tmp_path/"plan.snapshot.md").write_text(content)
    meta=call("command",operation="run-metadata",arguments={"directory":str(tmp_path)})
    assert (meta["title"],meta["total_tasks"],meta["repo"]) == ("Entregas do aplicativo móvel",1,"/fixture/project")
    plan.unlink()
    meta=call("command",operation="run-metadata",arguments={"directory":str(tmp_path)})
    assert meta["title"] == "Entregas do aplicativo móvel" and meta["error"] is None
    assert meta["total_tasks"] == 1 and meta["tasks"] == [{"n":1,"title":"Enviar mensagem"}]


@pytest.mark.parametrize("provider", ["claude", "codex"])
def test_history_usage_excludes_later_updates_in_the_same_transcript(tmp_path, provider):
    def record(at, count):
        if provider == "claude":
            return {"type": "assistant", "timestamp": at, "message": {"id": "response", "model": "claude-opus-5", "usage": {"input_tokens": count, "output_tokens": 2}}}
        return {"type": "event_msg", "timestamp": at, "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": count, "output_tokens": 2}}}}
    path = tmp_path / "transcript.jsonl"
    lines = [record("2026-09-30T10:00:00Z", 100), record("2026-09-30T12:00:00Z", 999)]
    path.write_text("".join(json.dumps(row) + "\n" for row in lines))
    rows = ot._rows_until(provider, str(path), datetime.fromisoformat("2026-09-30T11:00:00+00:00"))
    assert rows is not None and sum(r.input for r in rows) == 100
    assert sum(r.output for r in rows) == 2


def test_finished_run_never_resolves_a_reused_live_session(tmp_path, monkeypatch):
    result=call("consumption.compute",directory=tmp_path,names=["same-name"],aux={"since":"2026-09-30T10:00:00Z","until":"2026-09-30T11:00:00Z","models":{}},live=[{"name":"same-name"}],sources={},rows={})
    assert result["live"] == []
    assert result["value"]["sessions"]["missing"] == ["same-name"]
    assert result["value"]["totals"]["usd_partial"] is True


def test_history_usage_without_time_is_unavailable_instead_of_invented(tmp_path):
    path = tmp_path / "missing-time.jsonl"
    path.write_text(json.dumps({"type": "event_msg", "payload": {"type": "token_count", "info": {"total_token_usage": {"input_tokens": 900}}}}) + "\n")
    assert ot._rows_until("codex", str(path), datetime.fromisoformat("2026-09-30T11:00:00+00:00")) is None
