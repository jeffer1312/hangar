import json
import os
from datetime import timezone
from pathlib import Path

import pytest

from app import costs_claude_transcript as ct


def _escrever(p: Path, linhas: list[dict]) -> None:
    p.parent.mkdir(parents=True, exist_ok=True)
    p.write_text("\n".join(json.dumps(x) for x in linhas), encoding="utf-8")


def _turno(model: str, i: int, o: int, cw: int, cr: int, ts: str, sid: str = "s1") -> dict:
    return {"type": "assistant", "timestamp": ts, "sessionId": sid, "cwd": "/repo/a",
            "message": {"model": model, "usage": {
                "input_tokens": i, "output_tokens": o,
                "cache_creation_input_tokens": cw, "cache_read_input_tokens": cr}}}


@pytest.fixture(autouse=True)
def _limpo(tmp_path, monkeypatch):
    monkeypatch.setattr(ct, "_CACHE_DIR", tmp_path / "cache")
    ct.invalidar_cache()
    yield
    ct.invalidar_cache()


def _contador(monkeypatch):
    """Devolve (dict com a contagem). Contar chamadas ao parser é a ÚNICA forma de provar
    que o cache existe — a lição da fase 1: comparar só o resultado passa sem cache nenhum."""
    n = {"v": 0}
    real = ct.ler_transcript

    def contado(p):
        n["v"] += 1
        return real(p)

    monkeypatch.setattr(ct, "ler_transcript", contado)
    return n


def test_soma_os_turnos_da_sessao(tmp_path):
    """O transcript tem uso POR TURNO — a regra é SOMAR. O costs.jsonl era cumulativo e pedia
    a última linha; trocar a regra devolve número plausível e errado."""
    t = tmp_path / "proj" / "s1.jsonl"
    _escrever(t, [
        {"type": "user", "timestamp": "2026-07-01T10:00:00Z", "cwd": "/repo/a"},
        _turno("claude-opus-5", 10, 1, 5, 100, "2026-07-01T10:00:01Z"),
        _turno("claude-opus-5", 20, 2, 0, 200, "2026-07-01T10:05:00Z"),
    ])
    u = ct.ler_transcript(t)
    assert (u.input, u.output, u.cache_write, u.cache_read) == (30, 3, 5, 300)
    assert u.model == "claude-opus-5"
    assert u.cwd == "/repo/a"
    assert u.subagente is False


def test_turno_IGNORADO_nao_soma_nem_vira_ultimo_modelo(tmp_path):
    """A regressão crítica que a revisão final da fase 1 pegou, noutra forma. Um turno
    `<synthetic>` no FIM roubaria o slot de 'último modelo' e faria o linhas_claude descartar
    a sessão inteira. Medido no disco real: existe turno `<synthetic>` com usage zerado no
    meio de sessões normais."""
    t = tmp_path / "proj" / "s1.jsonl"
    _escrever(t, [
        _turno("claude-opus-5", 40, 7, 0, 900, "2026-07-01T10:00:00Z"),
        _turno("<synthetic>", 999, 999, 999, 999, "2026-07-01T10:01:00Z"),
    ])
    u = ct.ler_transcript(t)
    assert u.model == "claude-opus-5", "o turno ignorado não pode virar o modelo da sessão"
    assert (u.input, u.output, u.cache_read) == (40, 7, 900), "nem somar"


def test_sessao_so_com_turno_ignorado_e_none(tmp_path):
    t = tmp_path / "proj" / "s1.jsonl"
    _escrever(t, [_turno("<synthetic>", 5, 5, 5, 5, "2026-07-01T10:00:00Z")])
    assert ct.ler_transcript(t) is None


def test_ts_e_o_PRIMEIRO_turno(tmp_path):
    """A sessão pertence ao dia em que começou. Usar o último jogaria para o dia seguinte
    toda sessão que atravessa a meia-noite."""
    t = tmp_path / "proj" / "s1.jsonl"
    _escrever(t, [
        _turno("claude-opus-5", 1, 1, 0, 0, "2026-07-01T23:50:00Z"),
        _turno("claude-opus-5", 1, 1, 0, 0, "2026-07-02T00:10:00Z"),
    ])
    assert ct.ler_transcript(t).ts.astimezone(timezone.utc).strftime("%Y-%m-%d") == "2026-07-01"


def test_linha_invalida_e_tipo_errado_nao_derrubam(tmp_path):
    """`null` e lista são JSON válido e não levantam ValueError — exigir dict é obrigatório."""
    t = tmp_path / "proj" / "s1.jsonl"
    t.parent.mkdir(parents=True)
    t.write_text("\n".join(["{quebrado", "null", "[1,2]",
                            json.dumps(_turno("claude-opus-5", 7, 1, 0, 0, "2026-07-01T10:00:00Z"))]),
                 encoding="utf-8")
    assert ct.ler_transcript(t).input == 7


def test_subagente_entra_com_identidade_PROPRIA(tmp_path):
    """O transcript do subagente carrega o sessionId do PAI — medido: 168 de 446 ids repetidos
    entre arquivos. Usar esse campo como identidade repete o bug que o leitor do Pi já teve.
    E somar os dois NÃO duplica: medido num par real, o pai registra ZERO turnos na janela em
    que o filho roda."""
    _escrever(tmp_path / "proj" / "abc.jsonl",
              [_turno("claude-opus-5", 100, 0, 0, 0, "2026-07-01T10:00:00Z", sid="s1")])
    _escrever(tmp_path / "proj" / "abc" / "subagents" / "agent-x.jsonl",
              [_turno("claude-opus-5", 7, 0, 0, 0, "2026-07-01T10:00:30Z", sid="s1")])
    r = sorted(ct.varrer(tmp_path), key=lambda u: u.input)
    assert len(r) == 2, "o subagente é uma linha própria, não pode sumir nem fundir com o pai"
    assert r[0].session_id != r[1].session_id, "identidade vem do caminho, não do campo"
    assert (r[0].subagente, r[1].subagente) == (True, False)
    assert (r[0].input, r[1].input) == (7, 100)


def test_varrer_exige_raiz_e_cacheia(tmp_path, monkeypatch):
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 1, 0, 0, 0, "2026-07-01T10:00:00Z")])
    assert len(ct.varrer(tmp_path)) == 1
    n = _contador(monkeypatch)
    ct.varrer(tmp_path)
    assert n["v"] == 0, "arquivo inalterado não pode ser reparseado"


def test_cache_sobrevive_a_reinicio(tmp_path, monkeypatch):
    """É o ponto da tarefa: 13,6s de varredura fria não podem repetir a cada restart."""
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 5, 0, 0, 0, "2026-07-01T10:00:00Z")])
    ct.varrer(tmp_path)
    ct.invalidar_cache()          # simula processo novo: memória zerada, disco intacto
    n = _contador(monkeypatch)
    assert len(ct.varrer(tmp_path)) == 1
    assert n["v"] == 0, "o cache em DISCO tem que evitar o reparse depois do restart"


def test_duas_raizes_nao_apagam_o_cache_uma_da_outra(tmp_path, monkeypatch):
    """O app suporta mais de um diretório de configuração (é como o usuário mantém duas
    contas) e `coletar()` chama o leitor uma vez por diretório. Um cache que guardasse só a
    última raiz varrida nunca acertaria, e os 13,6s voltariam a cada request."""
    a, b = tmp_path / "A", tmp_path / "B"
    _escrever(a / "p" / "x.jsonl", [_turno("claude-opus-5", 1, 0, 0, 0, "2026-07-01T10:00:00Z")])
    _escrever(b / "p" / "y.jsonl", [_turno("claude-opus-5", 2, 0, 0, 0, "2026-07-01T10:00:00Z")])
    ct.varrer(a)
    ct.varrer(b)
    ct.invalidar_cache()
    n = _contador(monkeypatch)
    ct.varrer(a)
    ct.varrer(b)
    assert n["v"] == 0, "as duas raízes têm que continuar cacheadas"


def test_arquivo_alterado_e_relido(tmp_path):
    a = tmp_path / "p1" / "a.jsonl"
    _escrever(a, [_turno("claude-opus-5", 1, 0, 0, 0, "2026-07-01T10:00:00Z")])
    assert ct.varrer(tmp_path)[0].input == 1
    _escrever(a, [_turno("claude-opus-5", 9, 0, 0, 0, "2026-07-01T10:00:00Z")])
    os.utime(a, (0, 0))
    assert ct.varrer(tmp_path)[0].input == 9


def test_cache_de_versao_antiga_e_RELIDO(tmp_path, monkeypatch):
    """Discriminante: compara CHAMADAS ao parser, não o resultado — com o cache velho aceito
    o resultado seria o mesmo e o teste passaria sem provar nada."""
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 3, 0, 0, 0, "2026-07-01T10:00:00Z")])
    ct.varrer(tmp_path)
    p = ct._caminho_cache(tmp_path)
    d = json.loads(p.read_text(encoding="utf-8"))
    d["versao"] = ct.CACHE_VERSAO - 1
    p.write_text(json.dumps(d), encoding="utf-8")
    ct.invalidar_cache()
    n = _contador(monkeypatch)
    ct.varrer(tmp_path)
    assert n["v"] == 1, "versão velha tem que forçar releitura"


def test_cache_corrompido_nao_derruba(tmp_path):
    """JSON válido do tipo errado, e entrada com campo de tipo errado. Nenhum dos dois pode
    levantar — o pior caso aceitável é reler."""
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 4, 0, 0, 0, "2026-07-01T10:00:00Z")])
    ct.varrer(tmp_path)
    p = ct._caminho_cache(tmp_path)
    for lixo in ("null", "[1,2]", '{"versao": 1, "itens": {"x": {"sig": ["abc", 1]}}}'):
        p.write_text(lixo, encoding="utf-8")
        ct.invalidar_cache()
        assert len(ct.varrer(tmp_path)) == 1


def test_cache_com_bytes_invalidos_nao_derruba(tmp_path):
    """Achado da revisão: `UnicodeDecodeError` é subclasse de `ValueError`, não de `OSError` —
    um cache com bytes que não decodificam como UTF-8 (corrupção de disco, edição externa)
    propagava por `varrer()` até o chamador em vez de virar releitura."""
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 4, 0, 0, 0, "2026-07-01T10:00:00Z")])
    ct.varrer(tmp_path)
    p = ct._caminho_cache(tmp_path)
    p.write_bytes(b"\xff\xfe\x00lixo")
    ct.invalidar_cache()
    assert len(ct.varrer(tmp_path)) == 1


def test_falha_ao_gravar_cache_nao_derruba(tmp_path, monkeypatch):
    """Cache é otimização. Disco cheio ou diretório só-leitura tem que virar log, não 500 —
    regra do projeto: o núcleo nunca quebra por causa de uma feature."""
    _escrever(tmp_path / "p1" / "a.jsonl", [_turno("claude-opus-5", 6, 0, 0, 0, "2026-07-01T10:00:00Z")])

    def explode(*a, **kw):
        raise OSError("disco cheio")

    monkeypatch.setattr(ct, "_gravar_cache", explode)
    assert len(ct.varrer(tmp_path)) == 1
