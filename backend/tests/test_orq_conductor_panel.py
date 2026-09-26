"""Painel do condutor (app/orq_conductor.py) e as rotas GET /api/orq e /api/orq/{id}/conductor."""
import importlib.util
import json
import os
import subprocess
import sys
from datetime import datetime, timedelta, timezone
from pathlib import Path

import pytest
from fastapi.testclient import TestClient

from app import orq, orq_conductor
from app.config import settings

ORQ_PY = Path(__file__).resolve().parents[2] / "skills" / "orquestrar" / "scripts" / "orq.py"
AGORA = datetime.now().astimezone()
H = {"Authorization": "Bearer secret"}


def _iso(dt: datetime) -> str:
    return dt.isoformat(timespec="seconds")


def _registro(d: Path, *linhas: tuple[datetime, str]) -> None:
    (d / "registro.md").write_text("".join(f"- {_iso(t)} · {c}\n" for t, c in linhas), encoding="utf-8")


def _batimento(d: Path, idade_s: int, pid: int | None = None, intervalo: int = 60,
               agora: datetime = AGORA) -> None:
    (d / "vigia.json").write_text(json.dumps({
        "ts": _iso(agora - timedelta(seconds=idade_s)), "pid": pid or os.getpid(), "unit": "vigia-g1",
        "arbiter": "arb", "watching": ["rev1", "arb"], "states": {"rev1": "idle", "arb": "idle"},
        "interval_s": intervalo}), encoding="utf-8")


def _pid_morto() -> int:
    p = subprocess.Popen(["true"])
    p.wait()
    return p.pid


UNIDADE = {"Id": "vigia-g1.service", "ActiveState": "active",
           "ActiveEnterTimestamp": "@1790348806", "NRestarts": "2"}


# ── watchdog ────────────────────────────────────────────────────────────────

def test_batimento_novo_e_vivo(tmp_path):
    _batimento(tmp_path, idade_s=40)
    w = orq_conductor.watchdog(tmp_path, None, now=AGORA)
    assert w["alive"] is True and w["source"] == "heartbeat"
    assert w["arbiter"] == "arb" and w["watching"] == ["rev1", "arb"] and w["unit"] == "vigia-g1"
    assert w["since"] is None and w["restarts"] is None and w["unit_state"] is None


def test_batimento_velho_e_parado_desde_a_ultima_volta(tmp_path):
    _batimento(tmp_path, idade_s=151)   # 2 × 60 + 30 = 150 s
    w = orq_conductor.watchdog(tmp_path, None, now=AGORA)
    assert w["alive"] is False and w["last_cycle"] == _iso(AGORA - timedelta(seconds=151))


@pytest.mark.skipif(sys.platform != "linux", reason="a checagem do pid lê /proc")
def test_batimento_novo_de_processo_morto_e_parado(tmp_path):
    _batimento(tmp_path, idade_s=5, pid=_pid_morto())
    assert orq_conductor.watchdog(tmp_path, None, now=AGORA)["alive"] is False


def test_unidade_parada_derruba_batimento_novo(tmp_path):
    _batimento(tmp_path, idade_s=5)
    unidades = {os.path.normpath(str(tmp_path)): {**UNIDADE, "ActiveState": "inactive"}}
    w = orq_conductor.watchdog(tmp_path, unidades, now=AGORA)
    assert w["alive"] is False and w["unit_state"] == "inactive" and w["restarts"] == 2


def test_sem_batimento_vale_a_unidade(tmp_path):
    w = orq_conductor.watchdog(tmp_path, {os.path.normpath(str(tmp_path)): UNIDADE}, now=AGORA)
    assert w["alive"] is True and w["source"] == "systemd" and w["unit"] == "vigia-g1"
    assert w["since"] == datetime.fromtimestamp(1790348806).astimezone().isoformat(timespec="seconds")
    assert w["last_cycle"] is None and w["watching"] == []


def test_sem_nada_e_sem_condutor_e_sem_systemd_e_indisponivel(tmp_path):
    assert orq_conductor.watchdog(tmp_path, {}, now=AGORA)["source"] == "none"
    assert orq_conductor.watchdog(tmp_path, None, now=AGORA)["source"] == "unavailable"


# ── units (systemd) ─────────────────────────────────────────────────────────

SHOW = """Id=vigia-g1.service
ActiveState=active
ActiveEnterTimestamp=@1790348806
NRestarts=0
ExecStart={ path=/x/vigia.sh ; argv[]=/x/vigia.sh arb7 -e /home/u/.hangar/orq/2026-09-23-np -m 15 ; ignore_errors=no ; start_time=[@1790348806] ; stop_time=[n/a] ; pid=1 ; code=(null) ; status=0/0 }

Id=vigia-velho.service
ActiveState=failed
ActiveEnterTimestamp=
NRestarts=4
ExecStart={ path=/x/vigia.sh ; argv[]=/x/vigia.sh exec rev arb 5 ; ignore_errors=no }
"""


def _systemd(monkeypatch, stdout="", rc=0, exc=None):
    monkeypatch.setattr(orq_conductor.sys, "platform", "linux")
    monkeypatch.setattr(orq_conductor.shutil, "which", lambda n: "/usr/bin/systemctl")
    chamadas = []

    def run(cmd, **kw):
        chamadas.append((cmd, kw))
        if exc:
            raise exc
        return subprocess.CompletedProcess(cmd, rc, stdout, "")

    monkeypatch.setattr(orq_conductor.subprocess, "run", run)
    return chamadas


def test_units_acha_a_unidade_pelo_dir_do_menos_e(monkeypatch):
    chamadas = _systemd(monkeypatch, SHOW)
    u = orq_conductor.units()
    assert list(u) == ["/home/u/.hangar/orq/2026-09-23-np"]
    assert u["/home/u/.hangar/orq/2026-09-23-np"]["Id"] == "vigia-g1.service"
    cmd, kw = chamadas[0]
    assert cmd[:4] == ["systemctl", "--user", "show", "vigia-*"] and "--timestamp=unix" in cmd
    assert kw["timeout"] == orq_conductor.SYSTEMCTL_TIMEOUT_S


def test_units_sem_systemd_travado_ou_com_erro_e_none(monkeypatch):
    _systemd(monkeypatch, exc=subprocess.TimeoutExpired("systemctl", 3))
    assert orq_conductor.units() is None
    _systemd(monkeypatch, rc=1)
    assert orq_conductor.units() is None
    _systemd(monkeypatch, SHOW)
    monkeypatch.setattr(orq_conductor.shutil, "which", lambda n: None)
    assert orq_conductor.units() is None
    monkeypatch.setattr(orq_conductor.shutil, "which", lambda n: "/usr/bin/systemctl")
    monkeypatch.setattr(orq_conductor.sys, "platform", "win32")
    assert orq_conductor.units() is None


# ── feed ────────────────────────────────────────────────────────────────────

def test_feed_classifica_pelos_prefixos_e_pula_linha_torta(tmp_path):
    t = AGORA - timedelta(minutes=10)
    _registro(tmp_path,
              (t, "orq init: arbiter=arb repo=/r"),
              (t + timedelta(seconds=1), "notify → arbiter: T3 preciso da tela"),
              (t + timedelta(seconds=2), "(jev: no action) ok"),
              (t + timedelta(seconds=3), "aviso: [aviso] T3 binário congelado"),
              (t + timedelta(seconds=4), "alarm: [vigia] rev1 is stopped"),
              (t + timedelta(seconds=5), "notify FAILED: hangar-send arb failed (rc=3)"),
              (t + timedelta(seconds=6), "closed session: ex1 (Task 1 closed)"),
              (t + timedelta(seconds=7), "joined group: rev2"),
              (t + timedelta(seconds=8), "screen taken by ex1"))
    with (tmp_path / "registro.md").open("a", encoding="utf-8") as f:
        f.write("## anotação solta de execução antiga\n- sem-data · texto\n")
    r = orq_conductor.feed(tmp_path)
    assert [(i["kind"], i["text"], i["task"]) for i in r["feed"]] == [
        ("notice", "joined group: rev2", None),
        ("notice", "closed session: ex1 (Task 1 closed)", 1),
        ("alarm", "notify FAILED: hangar-send arb failed (rc=3)", None),
        ("alarm", "[vigia] rev1 is stopped", None),
        ("notice", "[aviso] T3 binário congelado", 3),
        ("dropped", "ok", None),
        ("woke", "T3 preciso da tela", 3),
    ]
    assert r["skipped"] == 2 and r["truncated"] is False
    assert len({i["id"] for i in r["feed"]}) == len(r["feed"])


def _sombra(d: Path, *linhas) -> None:
    with (d / "jev-shadow.jsonl").open("a", encoding="utf-8") as f:
        for l in linhas:
            f.write((l if isinstance(l, str) else json.dumps(l, ensure_ascii=False)) + "\n")


def test_feed_casa_a_sombra_pelo_texto_e_pelo_tempo(tmp_path):
    t = AGORA - timedelta(minutes=5)
    _registro(tmp_path, (t, "notify → arbiter: tela fechada ⏎ 41 de 60"),
              (t + timedelta(minutes=1), "notify → arbiter: outro"))
    _sombra(tmp_path,
            {"ts": _iso(t), "mode": "shadow", "alarm": False, "text": "tela fechada\n41 de 60",
             "choice": "nothing", "p": 0.97,
             "veto": {"context": 0.1, "user": 0.2, "problem": 0.5, "deviation": 0.1}, "would_drop": False},
            {"ts": _iso(t - timedelta(minutes=3)), "mode": "shadow", "alarm": False, "text": "outro",
             "choice": "act", "p": 0.0, "veto": {}, "would_drop": False},
            "{torto")
    r = orq_conductor.feed(tmp_path)
    outro, tela = r["feed"]
    assert outro["jev"] is None          # mesmo texto, fora da janela de tempo
    assert tela["jev"] == {"mode": "shadow", "choice": "nothing", "p": 0.97,
                           "veto": {"context": 0.1, "user": 0.2, "problem": 0.5, "deviation": 0.1},
                           "held": ["problem"], "would_drop": False, "error": None}
    assert r["skipped"] == 1


def test_veto_nan_da_sombra_nao_derruba_a_resposta(tmp_path):
    t = AGORA - timedelta(minutes=1)
    _registro(tmp_path, (t, "notify → arbiter: x"))
    (tmp_path / "jev-shadow.jsonl").write_text(
        '{"ts": "%s", "mode": "shadow", "text": "x", "choice": "nothing", "p": NaN, '
        '"veto": {"context": NaN, "user": 0.1}, "would_drop": false}\n' % _iso(t), encoding="utf-8")
    r = orq_conductor.feed(tmp_path)
    jev = r["feed"][0]["jev"]
    assert jev["p"] is None and jev["veto"] == {"context": None, "user": 0.1} and jev["held"] == ["context"]
    json.dumps(r, allow_nan=False)   # o que o JSONResponse do Starlette faz


def test_feed_junta_eventos_com_ts_sem_fuso_em_ordem(tmp_path):
    # O ts sem fuso é lido como hora local: montado a partir da hora local, a ordem vale em qualquer TZ.
    base = datetime(2026, 9, 25, 13, 0, tzinfo=timezone.utc)
    sem_fuso = base.astimezone().replace(tzinfo=None).isoformat(timespec="seconds")
    com_fuso = (base + timedelta(minutes=5)).astimezone(timezone(timedelta(hours=-3)))
    (tmp_path / "eventos.jsonl").write_text("\n".join([
        json.dumps({"ts": sem_fuso, "tipo": "task_inicio", "task": 1, "titulo": "t",
                    "executor": "ex", "par": "rev"}),
        json.dumps({"ts": _iso(com_fuso), "tipo": "veredito", "task": 1, "rodada": 1,
                    "resultado": "aprova", "sessao": "rev"}),
        json.dumps({"ts": "x", "tipo": "entrega", "task": 1, "rodada": 1}),
        "{torto",
    ]) + "\n", encoding="utf-8")
    r = orq_conductor.feed(tmp_path)
    assert [i["text"] for i in r["feed"]] == ["veredito T1 r1 resultado=aprova sessao=rev",
                                              "task_inicio T1 executor=ex par=rev"]
    assert all(i["kind"] == "event" and i["task"] == 1 for i in r["feed"])
    assert r["skipped"] == 2


def test_feed_skips_ts_at_calendar_edge(tmp_path):
    # Sem fuso vira hora local (ValueError em fuso negativo); com +14:00 a conversão dá OverflowError.
    (tmp_path / "eventos.jsonl").write_text("\n".join(
        json.dumps({"ts": ts, "tipo": "entrega", "task": 1, "rodada": 1})
        for ts in ["0001-01-01T00:00:00", "0001-01-01T00:00:00+14:00", "9999-12-31T23:59:59"]
    ) + "\n", encoding="utf-8")
    r = orq_conductor.feed(tmp_path)
    assert r["feed"] == [] and r["skipped"] == 3


def test_feed_corta_em_500_e_avisa(tmp_path):
    base = datetime(2026, 9, 25, 10, 0, tzinfo=AGORA.tzinfo)
    (tmp_path / "eventos.jsonl").write_text("".join(
        json.dumps({"ts": _iso(base + timedelta(seconds=i)), "tipo": "entrega", "task": 1,
                    "rodada": i + 1}) + "\n" for i in range(600)), encoding="utf-8")
    r = orq_conductor.feed(tmp_path)
    assert len(r["feed"]) == 500 and r["truncated"] is True
    assert r["feed"][0]["text"] == "entrega T1 r600"


def _orq_cli():
    spec = importlib.util.spec_from_file_location("orq_cli", ORQ_PY)
    m = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(m)
    return m


def test_limiar_do_veto_e_o_mesmo_do_orq():
    assert orq_conductor.JEV_VETO_P == _orq_cli().VETO_P


def test_texto_do_evento_e_o_mesmo_da_linha_do_orq():
    amostras = [
        {"ts": "t", "tipo": "execucao_inicio", "plano": "p", "branch": "b", "gid": "g1"},
        {"ts": "t", "tipo": "task_inicio", "task": 3, "titulo": "x", "executor": "ex", "par": "rev"},
        {"ts": "t", "tipo": "entrega", "task": 3, "rodada": 2, "commit": "abc123"},
        {"ts": "t", "tipo": "veredito", "task": 3, "rodada": 2, "resultado": "reprova",
         "sessao": "rev", "reincide": True},
        {"ts": "t", "tipo": "sessao_trocada", "de": "arb", "para": "arb2", "motivo": "contexto"},
        {"ts": "t", "tipo": "execucao_fim", "resultado": "aprova"},
    ]
    linha = _orq_cli()._event_line
    for ev in amostras:
        assert orq_conductor._event_text(ev) == linha(ev)


# ── rotas ───────────────────────────────────────────────────────────────────

@pytest.fixture
def cli(monkeypatch, tmp_path):
    monkeypatch.setattr(settings, "auth_token", "secret")
    monkeypatch.setattr(orq, "raiz_padrao", lambda: tmp_path)
    monkeypatch.setattr(orq_conductor, "units", lambda: {})
    from app.api import app
    return TestClient(app)


def test_rota_do_condutor_e_a_lista_com_o_chip(cli, tmp_path):
    d = tmp_path / "2026-09-25-g1"
    d.mkdir()
    (d / "eventos.jsonl").write_text(json.dumps({"ts": _iso(AGORA), "tipo": "execucao_inicio",
                                                 "plano": "p", "branch": "b", "gid": "g1"}) + "\n")
    # A rota lê o relógio real; AGORA é da importação e a suíte inteira passa da folga de 150 s.
    _batimento(d, idade_s=10, agora=datetime.now().astimezone())
    r = cli.get("/api/orq/2026-09-25-g1/conductor", headers=H)
    assert r.status_code == 200, r.text
    body = r.json()
    assert set(body) == {"watchdog", "feed", "truncated", "skipped"}
    assert body["watchdog"]["alive"] is True and body["watchdog"]["source"] == "heartbeat"
    assert [i["kind"] for i in body["feed"]] == ["event"]
    lista = cli.get("/api/orq", headers=H).json()
    assert lista["execucoes"][0]["watchdog"]["alive"] is True


def test_rota_do_condutor_guarda_o_caminho(cli):
    for ruim in ("a:b", "nada"):
        assert cli.get(f"/api/orq/{ruim}/conductor", headers=H).status_code == 404
