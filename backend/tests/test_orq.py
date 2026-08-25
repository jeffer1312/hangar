import json
from pathlib import Path

from app import orq


def _grava(raiz: Path, nome: str, linhas: list[dict]) -> None:
    d = raiz / nome
    d.mkdir(parents=True, exist_ok=True)
    (d / "eventos.jsonl").write_text(
        "\n".join(json.dumps(x) for x in linhas), encoding="utf-8")


def _exec_basica(gid="abc123"):
    return [
        {"ts": "2026-08-22T09:00:00-03:00", "tipo": "execucao_inicio",
         "plano": "docs/superpowers/plans/p.md", "branch": "mobile-expo", "gid": gid},
        {"ts": "2026-08-22T09:05:00-03:00", "tipo": "task_inicio", "task": 1,
         "titulo": "Ditado", "executor": "mx2-exec-t1", "par": "pi · muse-spark"},
        {"ts": "2026-08-22T11:00:00-03:00", "tipo": "entrega", "task": 1, "rodada": 1},
        {"ts": "2026-08-22T11:30:00-03:00", "tipo": "veredito", "task": 1, "rodada": 1,
         "resultado": "aprova", "sessao": "mx2-rev-t1", "commit": "aaa111"},
        {"ts": "2026-08-22T12:00:00-03:00", "tipo": "execucao_fim", "resultado": "concluida"},
    ]


def test_lista_uma_execucao_com_task_aprovada_de_primeira(tmp_path):
    _grava(tmp_path, "2026-08-22-paridade", _exec_basica())
    execs = orq.listar_execucoes(tmp_path)
    assert len(execs) == 1
    e = execs[0]
    assert e.id == "2026-08-22-paridade"
    assert e.resultado == "concluida"
    assert e.aprovadas_primeira == 1 and e.voltas == 0
    assert e.tasks[0].par == "pi · muse-spark" and e.tasks[0].rodadas == 1


def test_devolvido_conta_volta_e_nao_aprovada_de_primeira(tmp_path):
    linhas = _exec_basica()
    linhas[3:3] = [
        {"ts": "2026-08-22T10:00:00-03:00", "tipo": "veredito", "task": 1, "rodada": 1,
         "resultado": "devolvido", "sessao": "mx2-rev-t1", "motivo": "prova fabricada"},
        {"ts": "2026-08-22T10:40:00-03:00", "tipo": "entrega", "task": 1, "rodada": 2},
    ]
    linhas[5]["rodada"] = 2  # o veredito final (aprova) e da rodada 2
    _grava(tmp_path, "2026-08-22-paridade", linhas)
    e = orq.listar_execucoes(tmp_path)[0]
    assert e.voltas == 1 and e.aprovadas_primeira == 0
    assert e.tasks[0].rodadas == 2 and e.tasks[0].resultado == "aprova"


def test_rodada_desconhecida_nao_e_aprovada_de_primeira(tmp_path):
    # achado 6 do pass adversarial: sem campo rodada, rodadas=0 = "nao sei" — o KPI nao infla
    linhas = _exec_basica()
    del linhas[2]["rodada"]
    del linhas[3]["rodada"]
    _grava(tmp_path, "2026-08-22-paridade", linhas)
    e = orq.listar_execucoes(tmp_path)[0]
    assert e.tasks[0].rodadas == 0
    assert e.aprovadas_primeira == 0
    assert e.tasks[0].resultado == "aprova"


def test_campo_de_tipo_errado_nao_derruba(tmp_path):
    # achado 5: rodada null / task string sao JSON valido — nao podem virar TypeError
    linhas = _exec_basica()
    linhas.append({"ts": "2026-08-22T11:40:00-03:00", "tipo": "entrega",
                   "task": "7", "rodada": None})
    _grava(tmp_path, "2026-08-22-paridade", linhas)
    execs = orq.listar_execucoes(tmp_path)
    assert len(execs) == 1 and execs[0].tasks[0].rodadas == 1


def test_linha_quebrada_ignorada_e_dir_sem_linha_valida_fica_fora(tmp_path, caplog):
    _grava(tmp_path, "2026-08-22-paridade", _exec_basica())
    with open(tmp_path / "2026-08-22-paridade" / "eventos.jsonl", "a") as f:
        f.write("\n{json quebrado")
    _grava(tmp_path, "2026-08-16-so-lixo", [])
    (tmp_path / "2026-08-16-so-lixo" / "eventos.jsonl").write_text('{"tipo": "x"}')
    execs = orq.listar_execucoes(tmp_path)
    assert [e.id for e in execs] == ["2026-08-22-paridade"]
    assert "so-lixo" in caplog.text  # achado 8: ausencia com log, nunca muda


def test_sessao_trocada_sobrevive_no_nivel_da_execucao(tmp_path):
    # achado 7: evento sem task nao pode ser jogado fora
    linhas = _exec_basica()
    linhas.insert(2, {"ts": "2026-08-22T10:00:00-03:00", "tipo": "sessao_trocada",
                      "de": "mx2-exec-t1", "para": "mx2-exec-t1b", "motivo": "cota"})
    _grava(tmp_path, "2026-08-22-paridade", linhas)
    e = orq.listar_execucoes(tmp_path)[0]
    assert any(ev["tipo"] == "sessao_trocada" for ev in e.eventos_execucao)


def test_ordenacao_por_nome_de_diretorio(tmp_path):
    _grava(tmp_path, "2026-08-22-a", _exec_basica(gid="g1"))
    _grava(tmp_path, "2026-08-23-b", _exec_basica(gid="g2"))
    assert [e.id for e in orq.listar_execucoes(tmp_path)] == ["2026-08-23-b", "2026-08-22-a"]


def test_detalhe_inexistente_e_traversal_sao_none(tmp_path):
    assert orq.detalhe(tmp_path, "nao-existe") is None
    assert orq.detalhe(tmp_path, "../fora") is None


def test_ficha_conta_aceita_e_nao_aceita(tmp_path):
    _grava(tmp_path, "2026-08-22-a", _exec_basica(gid="g1"))
    linhas = _exec_basica(gid="g2")
    linhas[3]["resultado"] = "reprova"  # task abandonada em reprova
    _grava(tmp_path, "2026-08-23-b", linhas)
    f = orq.fichas(orq.listar_execucoes(tmp_path))
    assert f == [{"par": "pi · muse-spark", "aceitas": 1, "nao_aceitas": 1,
                  "aprovadas_primeira": 1, "rodadas_media": 1.0}]


def test_execucao_sem_fim_e_viva(tmp_path):
    _grava(tmp_path, "2026-08-25-viva", _exec_basica()[:-1])
    e = orq.listar_execucoes(tmp_path)[0]
    assert e.fim is None and e.resultado is None


def test_raiz_padrao_aponta_orq_retros():
    assert orq.raiz_padrao() == Path.home() / ".claude" / "orq-retros"
