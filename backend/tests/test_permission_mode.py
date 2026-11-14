"""Modo de permissão que o Python ainda lê: o padrão da conta e o modo fora do plano da sessão."""
import json
from collections import OrderedDict

import app.permission_mode as pm


def test_modo_da_conta_le_settings_e_cai_no_padrao_do_app(tmp_path):
    """Sem `defaultMode` na conta a sessão nasceria pedindo permissão por ferramenta."""
    conta = tmp_path / "conta"
    conta.mkdir()
    assert pm.modo_da_conta(str(conta)) == pm.PADRAO_DO_APP
    (conta / "settings.json").write_text('{"permissions": {"defaultMode": "plan"}}', encoding="utf-8")
    assert pm.modo_da_conta(str(conta)) == "plan"
    # "default" é o nome no settings.json; a flag da CLI só conhece "manual".
    (conta / "settings.json").write_text('{"permissions": {"defaultMode": "default"}}', encoding="utf-8")
    assert pm.modo_da_conta(str(conta)) == "manual"
    (conta / "settings.json").write_text("{ nao e json", encoding="utf-8")
    assert pm.modo_da_conta(str(conta)) == pm.PADRAO_DO_APP


def _transcript(tmp_path, nome, modos):
    p = tmp_path / f"{nome}.jsonl"
    p.write_text("".join(json.dumps({"type": "user", "permissionMode": m}) + "\n" for m in modos))
    return str(p)


def test_transcript_non_plan_mode_pula_plan(tmp_path):
    assert pm.transcript_non_plan_mode(_transcript(tmp_path, "a", ["default", "bypassPermissions", "plan"])) \
        == "bypassPermissions"
    assert pm.transcript_non_plan_mode(_transcript(tmp_path, "b", ["plan"])) is None
    assert pm.transcript_non_plan_mode(str(tmp_path / "sumiu.jsonl")) is None


def test_transcript_non_plan_mode_nao_rele_o_mesmo_transcript(tmp_path, monkeypatch):
    import app.worktrees as wt

    leituras = []
    original = wt.reversed_lines
    monkeypatch.setattr(wt, "reversed_lines", lambda path: leituras.append(path) or original(path))
    caminho = _transcript(tmp_path, "c", ["default", "acceptEdits"])
    assert pm.transcript_non_plan_mode(caminho) == "acceptEdits"
    assert pm.transcript_non_plan_mode(caminho) == "acceptEdits"
    assert len(leituras) == 1
    with open(caminho, "a") as f:
        f.write(json.dumps({"type": "user", "permissionMode": "bypassPermissions"}) + "\n")
    assert pm.transcript_non_plan_mode(caminho) == "bypassPermissions"
    assert len(leituras) == 2


def test_session_non_plan_mode_le_a_memoria_pelo_session_id(tmp_path, monkeypatch):
    # O monitor grava a memória pelo session-id (stem do jsonl); pelo nome da sessão ela não vale.
    monkeypatch.setattr(pm, "_ultimos_nao_plan", OrderedDict({"sess": "acceptEdits"}))
    jsonl = _transcript(tmp_path, "sid-1", ["bypassPermissions"])
    assert pm.session_non_plan_mode(jsonl) == "bypassPermissions"
    monkeypatch.setattr(pm, "_ultimos_nao_plan", OrderedDict({"sid-1": "acceptEdits"}))
    assert pm.session_non_plan_mode(jsonl) == "acceptEdits"
    assert pm.session_non_plan_mode(None) is None


def test_modo_de_abertura_le_o_registro_sem_atrapalhar_a_troca_do_rust(tmp_path, monkeypatch):
    # No Windows, ler o registro no meio do tmp+rename do Rust faz a troca dele falhar: a leitura
    # passa pelo `atomico.read`, que compartilha a exclusão.
    from app import atomico
    monkeypatch.setenv("HOME", str(tmp_path))
    specs = tmp_path / ".hangar" / "session-specs"
    specs.mkdir(parents=True)
    (specs / f"{'a' * 32}.json").write_text(json.dumps({"conversation": "sid-1", "permission_mode": "auto"}))
    lidos = []
    real = atomico.read
    monkeypatch.setattr(atomico, "read", lambda caminho: lidos.append(caminho) or real(caminho))
    assert pm._launch_mode("sid-1") == "auto"
    assert lidos == [specs / f"{'a' * 32}.json"]


def test_modo_de_abertura_e_o_do_registro_mais_recente_da_conversa(tmp_path, monkeypatch):
    # Fechar não apaga o registro e o Arquivo reabre a mesma conversa com outra chave: vale o mais novo.
    monkeypatch.setenv("HOME", str(tmp_path))
    specs = tmp_path / ".hangar" / "session-specs"
    specs.mkdir(parents=True)
    for chave, modo, criado in [("a", "acceptEdits", 20.0), ("b", "plan", 30.0), *((str(i), "bypassPermissions", float(i)) for i in range(1, 9))]:
        (specs / f"{chave * 32}.json").write_text(json.dumps({"conversation": "sid-1", "permission_mode": modo, "created_at": criado}))
    assert pm._launch_mode("sid-1") == "acceptEdits"
