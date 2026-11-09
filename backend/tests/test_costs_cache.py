"""Índice SQLite dos custos: leitura retomável do ponto onde parou, e releitura do zero só
quando o arquivo deixou de ser o mesmo."""
import json
import os
import sqlite3
from pathlib import Path

import pytest

from app import costs_cache as cc, costs_claude_transcript as ct, costs_sources as cs, pricing


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(cc, "_CACHE_DIR", tmp_path / "cache")
    monkeypatch.setattr(pricing, "_CACHE_DIR", tmp_path / "pricing")


def _resposta(rid: str, i: int, o: int, ts: str = "2026-09-10T12:00:00Z", cw: int = 0, cr: int = 0) -> dict:
    return {"type": "assistant", "timestamp": ts, "cwd": "/repo", "requestId": f"req-{rid}",
            "message": {"id": rid, "model": "claude-opus-5", "usage": {
                "input_tokens": i, "output_tokens": o,
                "cache_creation_input_tokens": cw, "cache_read_input_tokens": cr}}}


def _linhas(*ds) -> str:
    return "".join(json.dumps(d) + "\n" for d in ds)


def _anexar(p: Path, texto: str) -> None:
    with open(p, "a", encoding="utf-8") as f:
        f.write(texto)


def _novas(monkeypatch) -> dict:
    """Conta leituras do ZERO (dobra nova). Retomar do offset não cria dobra."""
    n = {"v": 0}
    original = ct._nova_dobra

    def contada(raiz):
        fabrica = original(raiz)

        def nova(p):
            n["v"] += 1
            return fabrica(p)
        return nova

    monkeypatch.setattr(ct, "_nova_dobra", contada)
    return n


def _soma(raiz: Path) -> tuple[int, int]:
    usos = ct.varrer(raiz)
    return sum(u.input for u in usos), sum(u.output for u in usos)


def test_anexo_e_lido_do_offset_e_da_o_mesmo_que_ler_do_zero(tmp_path, monkeypatch):
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 10, 1), _resposta("r2", 20, 2)), encoding="utf-8")
    assert _soma(raiz) == (30, 3)
    n = _novas(monkeypatch)
    _anexar(arq, _linhas(_resposta("r3", 40, 4, cw=500), _resposta("r4", 5, 5, cr=900)))
    assert _soma(raiz) == (75, 12)
    assert n["v"] == 0, "arquivo que só cresceu é lido a partir do offset"
    (inteiro,) = ct.ler_transcript(arq)
    (retomado,) = ct.varrer(raiz)
    assert (retomado.cache_write, retomado.cache_read, retomado.regravado) == (
        inteiro.cache_write, inteiro.cache_read, inteiro.regravado)


def test_blocos_da_mesma_resposta_em_coletas_diferentes_valem_a_ultima(tmp_path, monkeypatch):
    """O streaming grava a resposta em vários blocos com o usage crescendo; se a coleta cai no
    meio, o bloco seguinte chega na próxima e SUBSTITUI o anterior, sem somar."""
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 10, 2)), encoding="utf-8")
    assert _soma(raiz) == (10, 2)
    _anexar(arq, _linhas(_resposta("r1", 10, 12), _resposta("r2", 1, 1)))
    assert _soma(raiz) == (11, 13)


def test_linha_sem_quebra_conta_e_nao_duplica_quando_completa(tmp_path, monkeypatch):
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 1, 0)) + json.dumps(_resposta("r2", 2, 0)), encoding="utf-8")
    assert _soma(raiz) == (3, 0)
    n = _novas(monkeypatch)
    _anexar(arq, "\n" + _linhas(_resposta("r3", 4, 0)))
    assert _soma(raiz) == (7, 0)
    assert n["v"] == 0
    # Linha pela metade (escrita em andamento) não quebra nem vira uso.
    _anexar(arq, '{"type": "assistant", "timest')
    assert _soma(raiz) == (7, 0)


def test_arquivo_truncado_ou_substituido_e_relido_do_zero(tmp_path, monkeypatch):
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 100, 0), _resposta("r2", 200, 0)), encoding="utf-8")
    assert _soma(raiz) == (300, 0)
    n = _novas(monkeypatch)
    arq.write_text(_linhas(_resposta("r9", 7, 0)), encoding="utf-8")        # encolheu
    assert _soma(raiz) == (7, 0)
    assert n["v"] == 1
    novo = arq.with_suffix(".tmp")
    novo.write_text(_linhas(*(_resposta(f"x{k}", 1, 0) for k in range(5))), encoding="utf-8")
    os.replace(novo, arq)                                                     # outro inode, maior
    assert _soma(raiz) == (5, 0)
    assert n["v"] == 2
    arq.unlink()
    assert ct.varrer(raiz) == []


def test_reescrito_maior_no_mesmo_inode_e_relido(tmp_path, monkeypatch):
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 10, 0)), encoding="utf-8")
    assert _soma(raiz) == (10, 0)
    # Mesmo inode, maior, e o trecho antes do offset mudou.
    with open(arq, "r+", encoding="utf-8") as f:
        f.write(_linhas(_resposta("r1", 90, 0, cr=5), _resposta("r2", 1, 0)))
    assert _soma(raiz) == (91, 0)


def test_troca_do_mapa_de_areas_refaz_so_as_linhas_de_area(tmp_path, monkeypatch):
    from app import uso_areas
    monkeypatch.setattr(uso_areas, "_arquivo", lambda: tmp_path / "uso-areas.json")
    uso_areas.recarregar()
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    ler = _resposta("r1", 10, 1)
    ler["message"]["content"] = [{"type": "tool_use", "id": "t1", "name": "Read",
                                  "input": {"file_path": "/repo/src/app/page.ts"}}]
    arq.write_text(_linhas({"type": "user", "promptId": "p1", "cwd": "/repo", "timestamp": ler["timestamp"],
                            "message": {"content": "oi"}}, ler), encoding="utf-8")
    ct.varrer_uso(raiz)
    (tmp_path / "uso-areas.json").write_text(json.dumps(
        {"projetos": {"repo": [["front", ["src/app/*"]]]}}), encoding="utf-8")
    uso_areas.recarregar()
    n = _novas(monkeypatch)
    try:
        ct.varrer_uso(raiz)
        assert n["v"] == 0, "o mapa novo não relê o transcript"
    finally:
        uso_areas.recarregar()


def test_esquema_diferente_ou_banco_ilegivel_refaz_o_indice(tmp_path, monkeypatch):
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 3, 0)), encoding="utf-8")
    assert _soma(raiz) == (3, 0)
    monkeypatch.setattr(cc, "ESQUEMA", cc.ESQUEMA + 1)
    n = _novas(monkeypatch)
    assert _soma(raiz) == (3, 0)
    assert n["v"] == 1
    for lixo in (b"lixo que nao e banco", b"\xff\xfe\x00"):
        (cc._CACHE_DIR / cc._ARQUIVO).write_bytes(lixo)
        for sufixo in ("-wal", "-shm"):
            Path(f"{cc._CACHE_DIR / cc._ARQUIVO}{sufixo}").unlink(missing_ok=True)
        assert _soma(raiz) == (3, 0)


def test_sem_disco_o_indice_vai_pra_memoria(tmp_path, monkeypatch):
    """Índice é otimização: pasta que não pode ser criada vira log, não erro no relatório."""
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 6, 0)), encoding="utf-8")
    bloqueio = tmp_path / "arquivo"
    bloqueio.write_text("x", encoding="utf-8")
    monkeypatch.setattr(cc, "_CACHE_DIR", bloqueio / "cache")
    monkeypatch.setattr(cc, "_MEMORIA", f"file:hangar-custos-teste-{os.getpid()}?mode=memory&cache=shared")
    monkeypatch.setattr(cc, "_ancora", None)
    assert _soma(raiz) == (6, 0)


def test_rollout_codex_retoma_do_offset(tmp_path, monkeypatch):
    arq = tmp_path / "rollout-2026-09-10T12-00-00-x.jsonl"
    sid = "019a0000-0000-7000-8000-000000000001"

    def uso(rid, i, o):
        return {"type": "token_usage_record", "timestamp": "2026-09-10T12:00:00Z",
                "payload": {"thread_id": sid, "turn_id": "t1", "response_id": rid,
                            "usage": {"input_tokens": i, "cached_input_tokens": 0, "output_tokens": o}}}

    arq.write_text(_linhas({"type": "session_meta", "payload": {"id": sid, "cwd": "/r"}},
                           {"type": "turn_context", "payload": {"turn_id": "t1", "model": "gpt-5.6-sol"}},
                           uso("a", 10, 1)), encoding="utf-8")
    assert [(r.input, r.output) for r in cs.custos_do_rollout(arq)] == [(10, 1)]
    novas = []
    original = cs._dobra_codex
    monkeypatch.setattr(cs, "_dobra_codex", lambda p: novas.append(p) or original(p))
    _anexar(arq, _linhas(uso("b", 5, 2), uso("a", 10, 1)))   # "a" repetida não soma
    assert [(r.input, r.output) for r in cs.custos_do_rollout(arq)] == [(15, 3)]
    assert novas == []
    assert cs.custos_do_rollout(arq) == cs._linhas_rollout_codex(arq, None)


def test_rollout_sem_indice_devolve_none_e_o_gasto_da_sessao_segue_vazio(tmp_path, monkeypatch):
    from app import session_cost
    monkeypatch.setattr(cc, "sincronizar_arquivo", lambda *a, **k: None)
    arq = tmp_path / "rollout-x.jsonl"
    assert cs.custos_do_rollout(arq) is None
    assert session_cost._usage(arq) == ()


def test_estado_salvo_que_nao_serve_mais_e_relido_do_zero(tmp_path, monkeypatch):
    """Dobra que despickla mas quebra ao continuar (classe mudou sem subir a versão)."""
    import pickle
    import zlib
    raiz = tmp_path / "projects"
    arq = raiz / "p" / "s.jsonl"
    arq.parent.mkdir(parents=True)
    arq.write_text(_linhas(_resposta("r1", 10, 1)), encoding="utf-8")
    assert _soma(raiz) == (10, 1)
    conn = cc._abrir()
    conn.execute("UPDATE files SET estado=?", (zlib.compress(pickle.dumps(object())),))
    conn.close()
    _anexar(arq, _linhas(_resposta("r2", 5, 5)))
    assert _soma(raiz) == (15, 6)


def test_arquivo_gravado_fora_da_varredura_no_meio_dela_nao_quebra(tmp_path, monkeypatch):
    """O custo da sessão aberta insere o caminho enquanto a varredura já o lia como novo."""
    arq = tmp_path / "rollout-2026-09-10T12-00-00-x.jsonl"
    arq.write_text(_linhas({"type": "session_meta", "payload": {"id": "s1", "cwd": "/r"}}), encoding="utf-8")
    original = cc._ler_novo

    def no_meio(p, st, reg, nova, versao):
        lido = original(p, st, reg, nova, versao)
        monkeypatch.setattr(cc, "_ler_novo", original)
        cc.sincronizar_arquivo(p, cs._dobra_codex, "v", "codex:avulso")
        return lido

    monkeypatch.setattr(cc, "_ler_novo", no_meio)
    cc.sincronizar("codex:conta", [arq], cs._dobra_codex, "v")
    conn = cc._abrir()
    assert conn.execute("SELECT scope FROM files").fetchall() == [("codex:conta",)]
    conn.close()


def test_arquivo_apagado_de_escopo_que_saiu_da_varredura_e_esquecido(tmp_path):
    arq = tmp_path / "rollout-2026-09-10T12-00-00-x.jsonl"
    fica = tmp_path / "rollout-2026-09-10T12-00-00-y.jsonl"
    for p in (arq, fica):
        p.write_text(_linhas({"type": "session_meta", "payload": {"id": p.stem, "cwd": "/r"}}), encoding="utf-8")
        cc.sincronizar_arquivo(p, cs._dobra_codex, "v", "codex:avulso")
    arq.unlink()
    cc.esquecer_fora(set())
    conn = cc._abrir()
    assert conn.execute("SELECT path FROM files").fetchall() == [(str(fica),)]
    conn.close()


def test_relatorio_pronto_vale_ate_os_dados_mudarem(tmp_path):
    montados = []

    def montar():
        montados.append(1)
        return object()

    a = cc.relatorio(("x", 1), montar)
    assert cc.relatorio(("x", 1), montar) is a
    cc.relatorio(("x", 2), montar)
    assert len(montados) == 2
    cc.mudou()
    assert cc.relatorio(("x", 1), montar) is not a
    assert len(cc._relatorios) == 1, "entradas da versão velha saem"


def _rollout(tmp_path: Path, nome: str) -> Path:
    p = tmp_path / f"rollout-2026-09-10T12-00-00-{nome}.jsonl"
    p.write_text(_linhas({"type": "session_meta", "payload": {"id": nome, "cwd": "/r"}}), encoding="utf-8")
    return p


def _n_files() -> int:
    conn = cc._abrir()
    try:
        return conn.execute("SELECT count(*) FROM files").fetchone()[0]
    finally:
        conn.close()


def test_leitura_da_varredura_nao_segura_o_lock_de_escrita(tmp_path, monkeypatch):
    arqs = [_rollout(tmp_path, n) for n in ("x", "y")]
    original = cc._ler_novo
    livre = []

    def sonda(*a):
        outro = sqlite3.connect(cc._CACHE_DIR / cc._ARQUIVO, timeout=0, isolation_level=None)
        try:
            outro.execute("BEGIN IMMEDIATE")
            outro.execute("COMMIT")
            livre.append(True)
        except sqlite3.OperationalError:
            livre.append(False)
        finally:
            outro.close()
        return original(*a)

    monkeypatch.setattr(cc, "_ler_novo", sonda)
    cc.sincronizar("codex:conta", arqs, cs._dobra_codex, "v")
    assert livre == [True, True]
    assert _n_files() == 2


def test_indice_ocupado_nao_derruba_o_custo_da_sessao(tmp_path, monkeypatch):
    arq = _rollout(tmp_path, "x")
    assert cc.sincronizar_arquivo(arq, cs._dobra_codex, "v", "codex:avulso") is not None
    _anexar(arq, _linhas({"type": "event_msg", "payload": {"type": "outro"}}))
    original = cc._abrir

    def impaciente():
        conn = original()
        conn.execute("PRAGMA busy_timeout=50")
        return conn

    monkeypatch.setattr(cc, "_abrir", impaciente)
    dono = sqlite3.connect(cc._CACHE_DIR / cc._ARQUIVO, isolation_level=None)
    dono.execute("BEGIN IMMEDIATE")
    try:
        assert cc.sincronizar_arquivo(arq, cs._dobra_codex, "v", "codex:avulso") is None
    finally:
        dono.execute("ROLLBACK")
        dono.close()


def test_erro_passageiro_de_stat_nao_apaga_o_historico(tmp_path, monkeypatch):
    arq = _rollout(tmp_path, "x")
    cc.sincronizar("codex:conta", [arq], cs._dobra_codex, "v")
    real = os.stat

    def em_uso(p, *a, **kw):
        if str(p) == str(arq):
            raise PermissionError("em uso")
        return real(p, *a, **kw)

    monkeypatch.setattr(cc.os, "stat", em_uso)
    cc.sincronizar("codex:conta", [arq], cs._dobra_codex, "v")
    monkeypatch.setattr(cc.os, "stat", real)
    assert _n_files() == 1
    arq.unlink()
    cc.sincronizar("codex:conta", [arq], cs._dobra_codex, "v")
    assert _n_files() == 0, "sumiço de verdade continua esquecendo"
