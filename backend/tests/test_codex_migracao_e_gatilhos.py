"""Primeira rodada numa máquina com a ponte antiga, hook de estado próprio, interruptor e gatilho de sessão."""
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from app import codex_hook_installer, codex_integracao, runtime_config
from app.codex_compat import normalizar_hooks, wrapper_instalado
from app.codex_integracao import IntegracaoCodex, sem_hooks_do_app, sincronizacao_ligada

ESTADO = '"/repo/backend/.venv/bin/python" "/repo/backend/hooks/state_hook.py" || true'
RTK_PIPE = "rtk hook claude | python3 /repo/scripts/codex-hook-allow.py"
PESSOAL = "python3 /home/x/.claude/hooks/personal-hook.py"


def _grupo(*comandos, **extras):
    return {**extras, "hooks": [{"type": "command", "command": c} for c in comandos]}


def _home(tmp_path):
    (tmp_path / ".claude").mkdir()
    (tmp_path / ".codex").mkdir()
    (tmp_path / ".claude/settings.json").write_text('{"enabledPlugins": {}}')
    return tmp_path


# ------------------------------------------------------------- ponte antiga

def test_primeira_rodada_tira_so_o_que_o_instalador_antigo_escreveu(tmp_path):
    home = _home(tmp_path)
    espelho = {"hooks": {"SessionStart": [_grupo(PESSOAL), _grupo(ESTADO)],
                         "PreToolUse": [_grupo(RTK_PIPE, matcher="Bash")]}}
    depois_a_mao = "python3 /home/x/meu-hook.py"
    atual = {"hooks": {"SessionStart": [_grupo(PESSOAL), _grupo(ESTADO), _grupo(depois_a_mao)],
                       "PreToolUse": [_grupo(RTK_PIPE, matcher="Bash")]}}
    (home / ".codex/.hangar-hooks.json").write_text(json.dumps(espelho))
    (home / ".codex/hooks.json").write_text(json.dumps(atual))
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = codex_integracao._snapshot()
    service._migrar_ponte_antiga()
    novo = json.loads((home / ".codex/hooks.json").read_text())
    estado_novo = _grupo(codex_hook_installer.comando_estado())
    guarda_nova = _grupo(codex_hook_installer.comando_guarda(), matcher="Bash")
    assert novo == {"hooks": {"SessionStart": [_grupo(depois_a_mao), estado_novo],
                              "UserPromptSubmit": [estado_novo, _grupo(codex_hook_installer.comando_navegador())],
                              "PreToolUse": [estado_novo, guarda_nova],
                              "PostToolUse": [estado_novo], "Stop": [estado_novo]}}
    assert not (home / ".codex/.hangar-hooks.json").exists()
    assert list(service.backups.iterdir()), "o espelho e o hooks.json anterior vão pro backup"
    assert service._estado["confianca_pendente"] is True


def test_sem_espelho_nao_toca_no_hooks_json(tmp_path):
    home = _home(tmp_path)
    raw = json.dumps({"hooks": {"SessionStart": [_grupo(ESTADO)]}})
    (home / ".codex/hooks.json").write_text(raw)
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = codex_integracao._snapshot()
    service._migrar_ponte_antiga()
    assert (home / ".codex/hooks.json").read_text() == raw
    assert not service.raiz.exists()


# ------------------------------------------------------------- hooks do app

def test_hooks_do_app_nao_vao_pro_importador_os_do_usuario_vao():
    hooks = {"SessionStart": [_grupo(PESSOAL, ESTADO)],
             "Stop": [_grupo('"/x/backend/hooks/preview_hook.py" || exit 0')],
             "PreToolUse": [
                 _grupo("rtk hook claude", matcher="Bash"),
                 _grupo('"/venv/python" "/home/x/.claude/hooks/guard_tmux.py"', matcher="Bash"),
             ],
             "Estranho": "não é lista"}
    assert sem_hooks_do_app(hooks) == {"SessionStart": [_grupo(PESSOAL)],
                                       "PreToolUse": [_grupo("rtk hook claude", matcher="Bash")],
                                       "Estranho": "não é lista"}


@pytest.mark.parametrize("name", ["skill-suggester.py", "jev-command-gate.py", "jev-answer-check.py"])
@pytest.mark.parametrize("command", ['python3 /home/x/.claude/hooks/{}',
                                     r'& "C:\Python\python.exe" "C:\Users\x\.claude\hooks\{}" ; exit 0',
                                     'python3 "{}"'])
def test_claude_only_hooks_are_filtered_by_exact_basename(name, command):
    native = "python3 /home/x/.codex/hooks/jev-codex-observer.py"
    unknown = f"python3 /hooks/custom-{name}"
    backup = f"python3 /hooks/{name}.bak"
    hooks = {"PreToolUse": [_grupo(command.format(name), native, unknown, backup,
                                 matcher="Bash", timeout=10)],
             "Stop": [_grupo(command.format(name))]}
    expected = {"PreToolUse": [_grupo(native, unknown, backup, matcher="Bash", timeout=10)]}
    assert sem_hooks_do_app(hooks) == expected
    assert len(hooks["PreToolUse"][0]["hooks"]) == 4, "a fonte não é alterada"
    assert sem_hooks_do_app(expected) == expected


def test_filtered_hooks_remove_only_previously_imported_entries(tmp_path):
    home = _home(tmp_path)
    imported = {"hooks": {"PreToolUse": [_grupo(
        "python3 /home/x/.codex/hooks/skill-suggester.py",
        "python3 /home/x/.codex/hooks/jev-command-gate.py", PESSOAL, matcher="Bash")],
        "Stop": [_grupo("python3 /home/x/.codex/hooks/jev-answer-check.py")]}}
    native = _grupo("python3 /native/jev-answer-check.py",
                    "python3 /native/jev-codex-observer.py", matcher="Bash")
    current = {"hooks": {"PreToolUse": [*imported["hooks"]["PreToolUse"], native],
                         "Stop": imported["hooks"]["Stop"]}}
    path = home / ".codex/hooks.json"
    path.write_text(json.dumps(current))
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = codex_integracao._snapshot()
    record = {"hooks": imported}
    source = {"hooks": sem_hooks_do_app(imported["hooks"])}
    service._hooks(source, record)
    assert json.loads(path.read_text()) == {"hooks": {
        "PreToolUse": [_grupo(PESSOAL, matcher="Bash"), native]}}
    assert record["hooks"] == source
    assert service._estado["confianca_pendente"] is True
    raw, mtime = path.read_bytes(), path.stat().st_mtime_ns
    service._estado = codex_integracao._snapshot()
    service._hooks(source, record)
    assert (path.read_bytes(), path.stat().st_mtime_ns) == (raw, mtime)
    assert service._estado["confianca_pendente"] is False


def test_hook_import_policy_change_aguarda_sincronizacao_manual(tmp_path, monkeypatch):
    home = _home(tmp_path)
    monkeypatch.setattr(codex_integracao, "_REPO", tmp_path / "repo")
    service = IntegracaoCodex(home, home / ".codex")
    policy = codex_integracao._HOOK_IMPORT_POLICY
    monkeypatch.setattr(codex_integracao, "_HOOK_IMPORT_POLICY", b"")
    _registro(service, fingerprint=service.fingerprint())
    assert service.precisa_reconciliar() is False
    monkeypatch.setattr(codex_integracao, "_HOOK_IMPORT_POLICY", policy)
    assert service.precisa_reconciliar() is False
    _registro(service, fingerprint=service.fingerprint())
    assert service.precisa_reconciliar() is False


def test_instalador_acrescenta_uma_vez_e_nao_reescreve_entrada_existente(tmp_path):
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"SessionStart": [_grupo(ESTADO)],
                                                             "PreToolUse": [_grupo(PESSOAL)]}}))
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=False) == [
        "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]
    data = json.loads((codex / "hooks.json").read_text())
    assert data["hooks"]["SessionStart"] == [_grupo(ESTADO)], "formato antigo fica como está"
    assert [h["command"] for g in data["hooks"]["PreToolUse"] for h in g["hooks"]] == [
        PESSOAL, codex_hook_installer._STATE_COMMAND, codex_hook_installer.comando_guarda(False)]
    assert [h["command"] for g in data["hooks"]["UserPromptSubmit"] for h in g["hooks"]] == [
        codex_hook_installer._STATE_COMMAND, codex_hook_installer.comando_navegador(False)]
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=False) == []
    assert json.loads((codex / "hooks.json").read_text()) == data


@pytest.mark.parametrize("legacy_exists", [False, True])
def test_instalador_repara_so_python_legado_ausente_do_pacote(tmp_path, legacy_exists):
    root = tmp_path / ".hangar/current"
    python = root / ("python/python.exe" if os.name == "nt" else "python/bin/python3")
    legacy = root / ("backend/.venv/Scripts/python.exe" if os.name == "nt" else "backend/.venv/bin/python")
    python.parent.mkdir(parents=True)
    python.write_text("")
    if legacy_exists:
        legacy.parent.mkdir(parents=True)
        legacy.write_text("")
    scripts = root / "backend/hooks"
    scripts.mkdir(parents=True, exist_ok=True)
    for name in ("state_hook.py", "nav_hook.py", "guard_tmux.py"):
        (scripts / name).write_text("")
    codex = tmp_path / ".codex"
    codex.mkdir()
    old = f'"{legacy}" "{scripts / "guard_tmux.py"}"'
    if os.name == "nt":
        old = f"& {old} ; exit $LASTEXITCODE"
    path = codex / "hooks.json"
    path.write_text(json.dumps({"hooks":{"PreToolUse":[_grupo(old, matcher="Bash")]}}))
    source = """
import json
from pathlib import Path
from app.codex_hook_installer import ensure_codex_state_hook_installed
home=Path.home()/'.codex'
ensure_codex_state_hook_installed(home)
before=(home/'hooks.json').stat().st_mtime_ns
second=ensure_codex_state_hook_installed(home)
print(json.dumps({'second':second,'unchanged':before==(home/'hooks.json').stat().st_mtime_ns}))
"""
    env = {**os.environ,"HOME":str(tmp_path),"USERPROFILE":str(tmp_path),
           "CODEX_HOME":str(codex),"PYTHONPATH":str(Path(__file__).resolve().parents[1])}
    result = subprocess.run([sys.executable,"-c",source],env=env,cwd=tmp_path,
                            text=True,capture_output=True,timeout=15)
    assert result.returncode == 0, result.stderr
    guards = [h["command"] for g in json.loads(path.read_text())["hooks"]["PreToolUse"]
              for h in g["hooks"] if "guard_tmux.py" in h.get("command", "")]
    expected = old if legacy_exists else f'"{python}" "{scripts / "guard_tmux.py"}"'
    if os.name == "nt" and not legacy_exists:
        expected = f"& {expected} ; exit $LASTEXITCODE"
    assert guards == [expected]
    assert json.loads(result.stdout) == {"second":[],"unchanged":True}


def test_no_windows_o_hook_de_estado_vai_em_powershell_e_o_formato_cmd_e_reescrito(tmp_path):
    # Medido na VM (codex 0.154.0): hooks rodam no PowerShell 7; `"exe" "arg" || exit 0` e
    # UnexpectedToken e sai com 1 em todo evento. Formato antigo nosso e reescrito SO no Windows.
    ps = codex_hook_installer.comando_estado(windows=True)
    assert ps.startswith('& "') and ps.endswith('" ; exit 0') and "state_hook.py" in ps
    assert codex_hook_installer.comando_estado(windows=False) == codex_hook_installer._STATE_COMMAND
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"SessionStart": [_grupo(ESTADO)],
                                                             "PreToolUse": [_grupo(PESSOAL)]}}))
    gravados = codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=True)
    assert "SessionStart" in gravados
    data = json.loads((codex / "hooks.json").read_text())
    assert data["hooks"]["SessionStart"] == [_grupo(ps)], "formato cmd reescrito no Windows"
    assert [h["command"] for g in data["hooks"]["PreToolUse"] for h in g["hooks"]] == [
        PESSOAL, ps, codex_hook_installer.comando_guarda(True)]
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=True) == []
    # Forma PowerShell de OUTRO venv/checkout ja funciona: nao e reescrita (preserva a aprovacao).
    # Barras normais: o basename POSIX deste teste nao separa `\`; no Windows real os dois valem.
    outro = '& "D:/outro/python.exe" "D:/outro/hooks/state_hook.py" ; exit 0'
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"SessionStart": [_grupo(outro)]}}))
    gravados = codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=True)
    assert "SessionStart" not in gravados
    assert json.loads((codex / "hooks.json").read_text())["hooks"]["SessionStart"] == [_grupo(outro)]


def test_no_windows_o_guard_importado_e_substituido_pelo_hook_proprio_do_codex(tmp_path):
    codex = tmp_path / ".codex"
    codex.mkdir()
    antigo = f'"{sys.executable}" "C:/Users/x/.claude/hooks/guard_tmux.py"'
    (codex / "hooks.json").write_text(json.dumps({"hooks": {
        "PreToolUse": [_grupo(antigo, matcher="Bash")],
    }}))
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex, windows=True).count("PreToolUse") == 1
    data = json.loads((codex / "hooks.json").read_text())
    comandos = [h["command"] for g in data["hooks"]["PreToolUse"] for h in g["hooks"]]
    guard = [c for c in comandos if "guard_tmux.py" in c]
    assert len(guard) == 1
    assert guard[0].startswith('& "')
    assert guard[0].endswith('; exit $LASTEXITCODE')
    assert "backend/hooks/guard_tmux.py" in guard[0].replace("\\", "/")


def test_instalador_nao_zera_evento_que_nao_e_lista(tmp_path):
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"Stop": {"editado": "à mão"}}}))
    gravados = codex_hook_installer.ensure_codex_state_hook_installed(codex)
    assert "Stop" not in gravados
    assert json.loads((codex / "hooks.json").read_text())["hooks"]["Stop"] == {"editado": "à mão"}


def test_navegador_preserva_comando_existente_e_confianca(tmp_path):
    comando = 'python3 /outro/checkout/backend/hooks/nav_hook.py'
    confianca = {"pessoal": {"approved": True}}
    (tmp_path / "hooks.json").write_text(json.dumps({"hooks": {
        "state": confianca, "UserPromptSubmit": [_grupo(comando)],
    }}))
    codex_hook_installer.ensure_codex_state_hook_installed(tmp_path, windows=False)
    data = json.loads((tmp_path / "hooks.json").read_text())
    comandos = [h["command"] for g in data["hooks"]["UserPromptSubmit"] for h in g["hooks"]]
    assert comandos == [comando, codex_hook_installer._STATE_COMMAND]
    assert data["hooks"]["state"] == confianca
    assert codex_hook_installer.ensure_codex_state_hook_installed(tmp_path, windows=False) == []


def test_navegador_tem_comando_powershell_no_windows():
    comando = codex_hook_installer.comando_navegador(windows=True)
    assert comando.startswith('& "') and comando.endswith('" ; exit 0')
    assert "nav_hook.py" in comando


@pytest.mark.parametrize("conteudo", ["{ quebrado", '{"hooks": []}'])
def test_instalador_nao_clobra_hooks_json_estranho(tmp_path, conteudo):
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(conteudo)
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex) == []
    assert (codex / "hooks.json").read_text() == conteudo
    assert codex_hook_installer.ensure_codex_state_hook_installed(tmp_path / "nao-existe") == []


# ------------------------------------------------------------- rtk estável

def test_normalizacao_reusa_o_wrapper_ja_gravado():
    gravado = {"hooks": {"PreToolUse": [_grupo("/venv-a/bin/python3 /checkout-a/scripts/codex-hook-allow.py -- rtk hook claude",
                                              matcher="Bash")]}}
    assert wrapper_instalado(gravado) == ("/venv-a/bin/python3", "/checkout-a/scripts/codex-hook-allow.py")
    assert wrapper_instalado({"hooks": {"PreToolUse": [_grupo(RTK_PIPE)]}}) is None
    fonte = {"hooks": {"PreToolUse": [_grupo("rtk hook claude", matcher="Bash")]}}
    python, wrapper = wrapper_instalado(gravado)
    from pathlib import Path
    assert normalizar_hooks(fonte, python, Path(wrapper)) == gravado


def test_hooks_de_outro_checkout_nao_reescrevem_o_rtk_instalado(tmp_path, monkeypatch):
    home = _home(tmp_path)
    linha = "/venv-a/bin/python3 /checkout-a/scripts/codex-hook-allow.py -- rtk hook claude"
    raw = json.dumps({"hooks": {"PreToolUse": [_grupo(linha, matcher="Bash")]}})
    (home / ".codex/hooks.json").write_text(raw)
    monkeypatch.setattr(sys, "executable", "/venv-b/bin/python3")
    monkeypatch.setattr(codex_integracao, "_REPO", tmp_path / "checkout-b")
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = codex_integracao._snapshot()
    registro = {}
    service._hooks({"hooks": {"PreToolUse": [_grupo("rtk hook claude", matcher="Bash")]}}, registro)
    assert (home / ".codex/hooks.json").read_text() == raw
    assert service._estado["confianca_pendente"] is False
    assert registro["hooks"]["hooks"]["PreToolUse"][0]["hooks"][0]["command"] == linha


# ------------------------------------------------------------- interruptor

@pytest.fixture
def rc(tmp_path, monkeypatch):
    monkeypatch.setattr(runtime_config, "_backend_config_base", lambda: tmp_path)
    monkeypatch.delenv("CP_CODEX_SYNC_ENABLED", raising=False)
    return runtime_config


def test_interruptor_da_tela_e_o_kill_switch_desligam_os_gatilhos(rc, monkeypatch):
    assert sincronizacao_ligada() is True
    rc.aplicar({"codex_sync": False})
    assert sincronizacao_ligada() is False
    rc.aplicar({"codex_sync": True, "automations": False})
    assert sincronizacao_ligada() is False
    rc.aplicar({"automations": True})
    assert sincronizacao_ligada() is True
    monkeypatch.setenv("CP_CODEX_SYNC_ENABLED", "0")
    assert sincronizacao_ligada() is False


def _registro(service, *, fingerprint, estado="ok", ha_segundos=10, proxima_em=3600):
    import time
    from app.codex_integracao import _iso
    service.raiz.mkdir(parents=True, exist_ok=True)
    (service.raiz / "estado.json").write_text(json.dumps({
        "fingerprint": fingerprint,
        "status": {"estado": estado, "ultima_execucao": _iso(time.time() - ha_segundos),
                   "proxima_atualizacao": _iso(time.time() + proxima_em)}}))


def test_abrir_sessao_so_reconcilia_quando_algo_mudou(tmp_path, monkeypatch):
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex")
    monkeypatch.setattr(service, "fingerprint", lambda *a, **k: "fp-1")
    assert service.precisa_reconciliar() is True, "nunca rodou"
    _registro(service, fingerprint="fp-1")
    assert service.precisa_reconciliar() is False, "fonte igual, marketplace no prazo, última ok"
    _registro(service, fingerprint="fp-0")
    assert service.precisa_reconciliar() is False, "fonte mudou: atualização só pela sincronização manual"
    _registro(service, fingerprint="fp-1", proxima_em=-1)
    assert service.precisa_reconciliar() is False, "o relógio não invalida um preparo concluído"
    _registro(service, fingerprint="fp-1", estado="parcial", ha_segundos=10)
    assert service.precisa_reconciliar() is False, "falha recente espera a retentativa"
    _registro(service, fingerprint="fp-1", estado="parcial", ha_segundos=301)
    assert service.precisa_reconciliar() is True
    _registro(service, fingerprint="fp-1", estado="ocioso")
    assert service.precisa_reconciliar() is True, "rodada interrompida retoma"


async def test_gatilho_de_sessao_respeita_interruptor_e_cache(rc, tmp_path, monkeypatch):
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex")
    chamadas = []

    async def iniciar(motivo, forcar):
        chamadas.append((motivo, forcar))
        return {"estado": "executando"}

    monkeypatch.setattr(service, "iniciar", iniciar)
    monkeypatch.setattr(service, "fingerprint", lambda *a, **k: "fp-1")
    rc.aplicar({"codex_sync": False})
    await service.sessao()
    assert chamadas == [], "interruptor desligado"
    rc.aplicar({"codex_sync": True})
    _registro(service, fingerprint="fp-1")
    await service.sessao()
    assert chamadas == [], "nada mudou: não chama o Codex"
    _registro(service, fingerprint="fp-0")
    assert (await service.sessao())["estado"] == "ok"
    assert chamadas == [], "abrir não atualiza uma conta já preparada"


@pytest.mark.parametrize("changed_source", [False, True])
async def test_abertura_reutiliza_preparo_concluido_sem_reconciliar(rc, tmp_path, changed_source):
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex", binario="codex-unused-in-ready-account")
    rc.aplicar({"codex_sync": True})
    fingerprint = "changed-source" if changed_source else service.fingerprint()
    _registro(service, fingerprint=fingerprint, proxima_em=-86400)
    path = service.raiz / "estado.json"
    before = (path.read_bytes(), path.stat().st_mtime_ns)
    try:
        result = await service.sessao()
    finally:
        await service.fechar()
    assert result["estado"] == "ok"
    assert (path.read_bytes(), path.stat().st_mtime_ns) == before


async def test_erro_inesperado_nao_deixa_o_estado_preso_em_executando(tmp_path, monkeypatch):
    from unittest.mock import AsyncMock

    class Nativo:
        def __init__(self, *a): pass
        async def __aenter__(self): return self
        async def __aexit__(self, *a): pass

    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex", nativo=Nativo)
    monkeypatch.setattr(service, "_instrucoes", lambda *_a, **_k: None)
    monkeypatch.setattr(service, "_config", AsyncMock())
    monkeypatch.setattr(service, "_plugins", AsyncMock(side_effect=TypeError("shape inesperado")))
    estado = await service.reconciliar()
    assert estado["estado"] == "erro"
    assert estado["erros"][0]["codigo"] == "erro_falha_inesperada"
    assert estado["erros"][0]["params"] == {"tipo": "TypeError"}
    assert service.status()["estado"] == "erro", "o botão não fica preso"
    assert "shape" not in estado["erros"][0]["texto"], "detalhe só no log"


@pytest.mark.parametrize("resposta", [{"data": "x"}, {"data": [{"hooks": "nada"}]}, {"data": ["x"]}, "x"])
async def test_hooks_list_em_formato_desconhecido_vira_aviso(tmp_path, resposta):
    from unittest.mock import AsyncMock
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex")
    service._estado = codex_integracao._snapshot()
    codex = type("C", (), {"request": AsyncMock(return_value=resposta)})()
    await service._conferir_confianca(codex)
    assert any("não informou a confiança" in a for a in service._estado["avisos"])
    assert service._estado["erros"], "resposta inválida não confirma preparo completo"


async def test_md_solto_em_agents_e_ignorado_com_aviso_sem_derrubar_a_etapa(tmp_path, monkeypatch):
    from app.codex_importador import CodexNativoErro
    home = _home(tmp_path)
    (home / ".claude/agents").mkdir()
    (home / ".claude/agents/vision.md").write_text("---\nname: vision\n---\nvê imagens")
    (home / ".claude/agents/README.md").write_text("# só documentação")
    (home / ".claude/settings.json").write_text('{"enabledPlugins": {}, "hooks": {}, "env": {}}')

    class Importer:
        def __init__(self, stage, cx, binario, **kwargs):
            self.stage, self.cx = stage, cx
        async def __aenter__(self): return self
        async def __aexit__(self, *a): pass
        async def detectar(self):
            return [{"itemType": "SUBAGENTS", "details": {"subagents": [{"name": "vision"}]}}]
        async def importar(self, itens):
            (self.cx / "agents").mkdir(parents=True, exist_ok=True)
            (self.cx / "agents/vision.md").write_text("vision convertido")
            (self.cx / "hooks.json").write_text('{"hooks": {}}')
            return {"itemTypeResults": []}
        async def historicos_importacao(self):
            raise CodexNativoErro("sem histórico")

    service = IntegracaoCodex(home, home / ".codex", nativo=Importer)
    service._estado = codex_integracao._snapshot()
    service.raiz.mkdir(parents=True)
    from unittest.mock import AsyncMock
    monkeypatch.setattr(service, "_config", AsyncMock())
    registro = {"artefatos": {str(home / ".codex/agents/README.md"): {"hash": "antigo"},
                              str(home / ".codex/agents/README-notas.md"): {"hash": "parecido"}}}
    await service._fragmentos(Importer(None, None, None), {}, registro)
    assert (home / ".codex/agents/vision.md").read_text() == "vision convertido"
    assert any("README.md" in a and "ignorados" in a for a in service._estado["avisos"])
    assert str(home / ".codex/agents/README.md") in registro["artefatos"], "artefato do nome ignorado não é podado"
    assert str(home / ".codex/agents/README-notas.md") not in registro["artefatos"], "nome parecido não é congelado junto"


def test_status_traz_resumo_de_skills_do_manifesto(tmp_path):
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex")
    assert service.status()["skills"] == {"ponte": 0, "nativas": 0}
    service.raiz.mkdir(parents=True)
    (service.raiz / "estado.json").write_text(json.dumps({"skills": {
        "a": {"mode": "symlink"}, "b": {"mode": "copy"}, "c": {"mode": "native"}, "d": {"mode": "native"}}}))
    assert service.status()["skills"] == {"ponte": 2, "nativas": 2}


async def test_rodada_grava_assinatura_da_fonte(tmp_path, monkeypatch):
    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex", nativo=object)
    monkeypatch.setattr(service, "_instrucoes", lambda *_a, **_k: None)
    (home / ".claude/settings.json").write_text('{"enabledPlugins": {}}')
    await service.reconciliar()  # nativo=object não abre: a rodada termina em erro DEPOIS de ler a fonte
    registro = json.loads((service.raiz / "estado.json").read_text())
    assert registro["fingerprint"] == service.fingerprint()
    assert registro["status"]["estado"] == "erro"
