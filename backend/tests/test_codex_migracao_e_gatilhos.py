"""Primeira rodada numa máquina com a ponte antiga, hook de estado próprio, interruptor e gatilho de sessão."""
import json
import sys

import pytest

from app import codex_hook_installer, codex_integracao, runtime_config
from app.codex_compat import normalizar_hooks, wrapper_instalado
from app.codex_integracao import IntegracaoCodex, sem_hooks_do_app, sincronizacao_ligada

ESTADO = '"/repo/backend/.venv/bin/python" "/repo/backend/hooks/state_hook.py" || true'
RTK_PIPE = "rtk hook claude | python3 /repo/scripts/codex-hook-allow.py"
PESSOAL = "python3 /home/x/.claude/hooks/skill-suggester.py"


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
    estado_novo = _grupo(codex_hook_installer._STATE_COMMAND)
    assert novo == {"hooks": {"SessionStart": [_grupo(depois_a_mao), estado_novo],
                              "UserPromptSubmit": [estado_novo], "PreToolUse": [estado_novo],
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
             "PreToolUse": [_grupo("rtk hook claude", matcher="Bash")],
             "Estranho": "não é lista"}
    assert sem_hooks_do_app(hooks) == {"SessionStart": [_grupo(PESSOAL)],
                                       "PreToolUse": [_grupo("rtk hook claude", matcher="Bash")],
                                       "Estranho": "não é lista"}


def test_instalador_acrescenta_uma_vez_e_nao_reescreve_entrada_existente(tmp_path):
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"SessionStart": [_grupo(ESTADO)],
                                                             "PreToolUse": [_grupo(PESSOAL)]}}))
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex) == [
        "UserPromptSubmit", "PreToolUse", "PostToolUse", "Stop"]
    data = json.loads((codex / "hooks.json").read_text())
    assert data["hooks"]["SessionStart"] == [_grupo(ESTADO)], "formato antigo fica como está"
    assert [h["command"] for g in data["hooks"]["PreToolUse"] for h in g["hooks"]] == [
        PESSOAL, codex_hook_installer._STATE_COMMAND]
    assert codex_hook_installer.ensure_codex_state_hook_installed(codex) == []
    assert json.loads((codex / "hooks.json").read_text()) == data


def test_instalador_nao_zera_evento_que_nao_e_lista(tmp_path):
    codex = tmp_path / ".codex"
    codex.mkdir()
    (codex / "hooks.json").write_text(json.dumps({"hooks": {"Stop": {"editado": "à mão"}}}))
    gravados = codex_hook_installer.ensure_codex_state_hook_installed(codex)
    assert "Stop" not in gravados
    assert json.loads((codex / "hooks.json").read_text())["hooks"]["Stop"] == {"editado": "à mão"}


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
    assert service.precisa_reconciliar() is True, "fonte mudou"
    _registro(service, fingerprint="fp-1", proxima_em=-1)
    assert service.precisa_reconciliar() is True, "marketplace venceu"
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
    assert (await service.sessao())["estado"] == "executando"
    assert chamadas == [("sessao", False)]


async def test_erro_inesperado_nao_deixa_o_estado_preso_em_executando(tmp_path, monkeypatch):
    from unittest.mock import AsyncMock

    class Nativo:
        def __init__(self, *a): pass
        async def __aenter__(self): return self
        async def __aexit__(self, *a): pass

    home = _home(tmp_path)
    service = IntegracaoCodex(home, home / ".codex", nativo=Nativo)
    monkeypatch.setattr(service, "_instrucoes", lambda: None)
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


async def test_md_solto_em_agents_e_ignorado_com_aviso_sem_derrubar_a_etapa(tmp_path, monkeypatch):
    from app.codex_importador import CodexNativoErro
    home = _home(tmp_path)
    (home / ".claude/agents").mkdir()
    (home / ".claude/agents/vision.md").write_text("---\nname: vision\n---\nvê imagens")
    (home / ".claude/agents/README.md").write_text("# só documentação")
    (home / ".claude/settings.json").write_text('{"enabledPlugins": {}, "hooks": {}, "env": {}}')

    class Importer:
        def __init__(self, stage, cx, binario):
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
    (home / ".claude/settings.json").write_text('{"enabledPlugins": []}')  # inválido: rodada em erro
    await service.reconciliar()
    registro = json.loads((service.raiz / "estado.json").read_text())
    assert registro["fingerprint"] == service.fingerprint()
    assert registro["status"]["estado"] == "erro"
