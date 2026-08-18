"""Testes da poda de sidecars (Task G3).

Casos que a Task manda cobrir, com o criterio conservador de app/prune.py:
  * sidecar de sessao VIVA nunca some (a chave manda, nao a idade);
  * sidecar VELHO de sessao morta some (>= _MIN_AGE);
  * sidecar NOVO de sessao morta NAO some (< _MIN_AGE) — decisao registrada no reporte:
    preserva a materia-prima de diagnostico de execucao morta recente (o achado do custo
    errado foi lido de sidecar de sessao morta), e a leitura ja recusa sidecar velho.
"""
import json
import os
from pathlib import Path
from unittest.mock import patch

from app import prune
from app.models import SessionInfo

_AGORA = 1_800_000_000.0


def _mtime(p: Path, agora: float, dias: float) -> None:
    os.utime(p, (agora - dias * 86400, agora - dias * 86400))


def _escreve(base: Path, sub: str, chave: str, agora: float, dias: float,
             suffix: str = ".json") -> Path:
    d = base / sub
    d.mkdir(parents=True, exist_ok=True)
    f = d / f"{chave}{suffix}"
    f.write_text(json.dumps({"x": 1}), encoding="utf-8")
    _mtime(f, agora, dias)
    return f


def _infos(*chaves) -> list[SessionInfo]:
    """SessionInfo fakes; a chave termina em '-kimi' -> jsonl de wire (session_key = sessionDir)."""
    out = []
    for k in chaves:
        if k.endswith("-kimi"):
            jsonl = f"/kimi/sessions/wd/{k}/agents/main/wire.jsonl"
        else:
            jsonl = f"/claude/projects/x/{k}.jsonl"
        out.append(SessionInfo(name=k, cwd="/x", jsonl=jsonl))
    return out


def _total(apagados: dict[str, int]) -> int:
    return sum(apagados.values())


# ── _podar: a regra em si ──────────────────────────────────────────────────────

def test_viva_nao_some_mesmo_velha(tmp_path):
    """Sidecar de sessao viva NAO e apagado, por mais velho que seja: a chave manda."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-status", "aaa", _AGORA, 60)     # 60 dias
    _escreve(base, ".claude-pocket-queue", "sessao", _AGORA, 60, ".jsonl")
    _escreve(base, ".claude-pocket-pi", "42", _AGORA, 60)
    apagados = prune._podar([base], {"aaa"}, {"sessao"}, {"42"}, _AGORA)
    assert _total(apagados) == 0


def test_morta_velha_some(tmp_path):
    """Sidecar de sessao morta com >= _MIN_AGE some, em todos os tipos de chave."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-state", "morto-stem", _AGORA, 8)
    _escreve(base, ".claude-pocket-pi/models", "morto-model", _AGORA, 8)
    _escreve(base, ".claude-pocket-queue", "morto-nome", _AGORA, 8, ".jsonl")
    _escreve(base, ".claude-pocket-pi", "999", _AGORA, 8)
    _escreve(base, ".claude-pocket-kimi", "888", _AGORA, 8)
    apagados = prune._podar([base], set(), set(), set(), _AGORA)
    assert apagados[".claude-pocket-state"] == 1
    assert apagados[".claude-pocket-pi/models"] == 1
    assert apagados[".claude-pocket-queue"] == 1
    assert apagados[".claude-pocket-pi"] == 1
    assert apagados[".claude-pocket-kimi"] == 1
    assert not (base / ".claude-pocket-state" / "morto-stem.json").exists()


def test_morta_recente_nao_some(tmp_path):
    """Sidecar de sessao morta RECENTE nao some: a poda preserva a materia-prima de
    diagnostico — decisao da Task, registrada no reporte (leitura ja recusa velho)."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-status", "morreu-ontem", _AGORA, 1)
    assert _total(prune._podar([base], set(), set(), set(), _AGORA)) == 0


def test_borda_exata(tmp_path):
    """A borda e >= _MIN_AGE: exatamente na idade cai; um segundo antes, nao."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-status", "na-borda", _AGORA, prune._MIN_AGE / 86400)
    assert prune._podar([base], set(), set(), set(), _AGORA)[".claude-pocket-status"] == 1
    _escreve(base, ".claude-pocket-status", "quase", _AGORA, prune._MIN_AGE / 86400 - 1e-6)
    assert _total(prune._podar([base], set(), set(), set(), _AGORA)) == 0


def test_arquivo_sem_stat_nao_derruba(tmp_path):
    base = tmp_path / "cfg"
    f = _escreve(base, ".claude-pocket-status", "sumiu", _AGORA, 30)
    f.unlink()  # some entre o glob e o stat
    assert _total(prune._podar([base], set(), set(), set(), _AGORA)) == 0


def test_tmp_meio_escrito_nao_casa(tmp_path):
    """O .tmp do publisher (escrita atomica) nunca e apanhado pelo glob."""
    base = tmp_path / "cfg"
    d = base / ".claude-pocket-status"
    d.mkdir(parents=True)
    (d / "aaa.json.tmp.123").write_text("x", encoding="utf-8")
    (d / "aaa.jsonl").write_text("x", encoding="utf-8")
    assert _total(prune._podar([base], set(), set(), set(), _AGORA)) == 0


# ── prune_sidecars: chaves resolvidas dos infos ───────────────────────────────

def test_prune_usa_session_key_kimi(tmp_path):
    """A chave do Kimi e o sessionDir (session_key do wire.jsonl), nao 'wire'."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-state", "vivo-kimi", _AGORA, 30)
    _escreve(base, ".claude-pocket-state", "morto-kimi", _AGORA, 30)
    infos = _infos("vivo-kimi")
    with patch.object(prune, "_pane_ids_vivos", return_value=set()):
        apagados = prune.prune_sidecars(infos=infos, agora=_AGORA, bases=[base])
    assert apagados[".claude-pocket-state"] == 1 and _total(apagados) == 1  # so o morto caiu


def test_prune_sanitiza_nome_da_fila(tmp_path):
    """O arquivo da fila e o nome SANITIZADO; nome vivo com caractere invalido protege o
    arquivo sanitizado correspondente."""
    base = tmp_path / "cfg"
    _escreve(base, ".claude-pocket-queue", "minha-sessao", _AGORA, 30, ".jsonl")
    infos = _infos("minha/sessao")
    with patch.object(prune, "_pane_ids_vivos", return_value=set()):
        apagados = prune.prune_sidecars(infos=infos, agora=_AGORA, bases=[base])
    assert _total(apagados) == 0


def test_prune_sem_infos_chama_registry_de_verdade(tmp_path, monkeypatch):
    """Sem infos, resolve ao vivo (SessionRegistry.list) — provado com spy, sem tocar disco."""
    chamou = []

    class _Fake:
        @staticmethod
        def list():
            chamou.append(1)
            return _infos("vivo-1")

    import app.registry as registry_mod

    monkeypatch.setattr(registry_mod, "SessionRegistry", _Fake)
    with patch.object(prune, "_pane_ids_vivos", return_value=set()):
        prune.prune_sidecars(agora=_AGORA, bases=[tmp_path / "cfg-vazio"])
    assert chamou == [1]
