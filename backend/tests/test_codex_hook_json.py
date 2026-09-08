"""Saídas medidas do security-guidance, sem SDK, rede ou configuração real."""
import json
from pathlib import Path
import shlex
import subprocess
import sys

import pytest

WRAPPER = Path(__file__).resolve().parents[2] / "scripts/codex-hook-json.py"


def rodar(tmp_path, saida, *, rc=0):
    script = tmp_path / "hook com espaço.py"
    script.write_text("import sys\nassert sys.stdin.buffer.read() == b'entrada'\n"
                      f"sys.stdout.buffer.write({saida!r})\n"
                      f"sys.stderr.write('diagnóstico')\nsys.exit({rc})\n", encoding="utf-8")
    command = subprocess.list2cmdline([sys.executable, str(script)]) if sys.platform == "win32" else shlex.join([sys.executable, str(script)])
    return subprocess.run([sys.executable, str(WRAPPER), "--", command], input=b"entrada", capture_output=True, timeout=5)


def test_bootstrap_assincrono_e_metricas_viram_resposta_valida(tmp_path):
    r = rodar(tmp_path, b'{"async":true,"asyncTimeout":180000}\n{"metrics":{"sdk_bootstrap":0}}\n')
    assert r.returncode == 0, r.stderr
    assert json.loads(r.stdout) == {}


def test_bloqueio_mensagem_e_stderr_nao_sao_descartados(tmp_path):
    dados = {"metrics": {"vulns_found": 1}, "rewakeSummary": "Revise a alteração",
             "decision": "block", "reason": "Problema encontrado", "systemMessage": "Confira"}
    r = rodar(tmp_path, json.dumps(dados).encode(), rc=2)
    assert r.returncode == 2
    assert r.stderr.decode() == "diagnóstico"
    assert json.loads(r.stdout) == {"decision": "block", "reason": "Problema encontrado", "systemMessage": "Confira"}


@pytest.mark.parametrize("saida", [b"", b"texto", b"\xff", b"null", b'{"continue":false,"stopReason":"pare"}',
                                  b'{"decision":"block","reason":"a"}\n{"decision":"block","reason":"b"}'])
def test_saida_desconhecida_ou_sem_telemetria_passa_intacta(tmp_path, saida):
    r = rodar(tmp_path, saida)
    assert r.returncode == 0
    assert r.stdout == saida


def test_contexto_e_resumo_preservados(tmp_path):
    contexto = {"hookEventName": "SessionStart", "additionalContext": "Instruções"}
    r = rodar(tmp_path, json.dumps({"metrics": {}, "hookSpecificOutput": contexto, "rewakeSummary": "Aviso"}).encode())
    assert json.loads(r.stdout) == {"hookSpecificOutput": contexto, "systemMessage": "Aviso"}


@pytest.mark.parametrize("nome", ["security-guidance", "outro-plugin"])
def test_plugin_alvo_e_adaptado_sem_reescrever_em_novo_checkout(tmp_path, nome):
    from app.codex_integracao import IntegracaoCodex
    service = IntegracaoCodex(tmp_path, tmp_path / ".codex")
    service._estado = {"avisos": [], "confianca_pendente": False}
    root = service.codex_home / "plugins/cache/market" / nome / "1"
    (root / ".codex-plugin").mkdir(parents=True)
    (root / ".codex-plugin/plugin.json").write_text(json.dumps({"name": nome}))
    programa = root / "hook.py"
    programa.write_text("print('{\"metrics\":{\"no_changes\":true}}')\n")
    command = subprocess.list2cmdline([sys.executable, str(programa)]) if sys.platform == "win32" else shlex.join([sys.executable, str(programa)])
    hooks = root / "hooks.json"
    hooks.write_text(json.dumps({"hooks": {"Stop": [{"hooks": [{"type": "command", "command": command}]}]}}))
    service._hooks_plugin(root)
    convertido = json.loads(hooks.read_text())["hooks"]["Stop"][0]["hooks"][0]["command"]
    r = subprocess.run(convertido, shell=True, input=b"{}", capture_output=True, timeout=5)
    assert r.returncode == 0, r.stderr
    if nome == "security-guidance":
        assert json.loads(r.stdout) == {}
        assert service._estado["confianca_pendente"] is True
        assert str(service.codex_home / ".hangar-hooks/codex-hook-json.py") in convertido
    else:
        assert convertido == command
        assert json.loads(r.stdout) == {"metrics": {"no_changes": True}}
    antes = hooks.read_bytes(), hooks.stat().st_mtime_ns
    service._hooks_plugin(root)
    assert (hooks.read_bytes(), hooks.stat().st_mtime_ns) == antes


def test_wrapper_windows_preserva_comando_e_interpretador_ja_instalado():
    from app.codex_compat import _split_windows, normalizar_security_guidance
    command = 'bash "C:\\plugin com espaço\\hooks\\sg-python.sh" "C:\\plugin com espaço\\hooks\\ensure_agent_sdk.py"'
    config = {"hooks": {"SessionStart": [{"hooks": [{"type": "command", "command": command}]}]}}
    wrapper = Path("C:/codex/.hangar-hooks/codex-hook-json.py")
    normal = normalizar_security_guidance(config, "C:/Python antigo/python.exe", wrapper, windows=True)
    cmd = normal["hooks"]["SessionStart"][0]["hooks"][0]["command"]
    assert _split_windows(cmd) == ["C:/Python antigo/python.exe", str(wrapper), "--", command]
    assert normalizar_security_guidance(normal, "C:/Python novo/python.exe", wrapper, windows=True) == normal
