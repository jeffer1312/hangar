"""Compatibilidade de hooks sem acesso às configurações reais do usuário."""

import json
import os
from pathlib import Path
import shlex
import shutil
import subprocess
import sys

import pytest

from app.codex_compat import (
    FIM_INSTRUCOES,
    INICIO_INSTRUCOES,
    _split_windows,
    normalizar_hooks,
    remover_instrucao_de_leitura,
)


@pytest.mark.parametrize("failure", [None,"exit","timeout"])
def test_runtime_ecc_instala_so_producao_uma_vez_antes_de_confiar(tmp_path, monkeypatch, failure):
    from app import codex_compat as module
    home = tmp_path / ".codex"
    root = home / "plugins/cache/ecc/ecc/1"
    (root / ".codex-plugin").mkdir(parents=True)
    (root / ".codex-plugin/plugin.json").write_text(json.dumps({"name":"ecc"}))
    (root / "package.json").write_text(json.dumps({"dependencies":{"js-yaml":"4.3.2"},
                                                  "devDependencies":{"not-needed":"1.0.0"}}))
    (root / "package-lock.json").write_text(json.dumps({"packages":{
        "node_modules/js-yaml":{"version":"4.3.2"}}}))
    node = tmp_path / "node"
    npm = tmp_path / "npm-cli.js"
    node.write_text("")
    npm.write_text("")
    monkeypatch.setattr(module.shutil, "which", lambda name: str(node if name == "node" else npm))
    calls = []

    def run(args, **kwargs):
        if args[1] != "-e":
            calls.append(args)
            if failure == "timeout":
                raise subprocess.TimeoutExpired(args,120,output=b"npm partial",stderr=b"private detail")
            if failure == "exit":
                return subprocess.CompletedProcess(args,1,stdout=b"npm partial",stderr=b"private detail")
            package = root / "node_modules/js-yaml"
            package.mkdir(parents=True)
            (package / "package.json").write_text('{"version":"4.3.2"}')
        return subprocess.CompletedProcess(args,0,stdout=b"",stderr=b"")

    monkeypatch.setattr(module.subprocess, "run", run)
    changed = []
    if failure:
        with pytest.raises(RuntimeError,match="dependências do ECC"):
            module.adaptar_hooks_plugin(root,home,tmp_path/"backups",normalizar=lambda value:value,
                                       ao_alterar=lambda:changed.append(True))
        assert b"private detail" in (tmp_path/"backups/ecc-runtime-npm.log").read_bytes()
        assert not changed
        return
    for _ in range(2):
        module.adaptar_hooks_plugin(root,home,tmp_path / "backups",normalizar=lambda value:value,
                                   ao_alterar=lambda:changed.append(True))
    assert (root / "node_modules/js-yaml/package.json").is_file()
    assert len(calls) == 1
    assert "--omit=dev" in calls[0]
    assert "--ignore-scripts" in calls[0]
    assert "--no-bin-links" in calls[0]
    assert changed == [True]


def test_confianca_inicial_aprova_so_hooks_da_conta_e_plugins(tmp_path):
    from app.codex_compat import edicoes_confianca_hooks
    data = {"data":[{"hooks":[
        {"key":"ecc@ecc:session_start:0:0","currentHash":"sha256:decl/sha256:content",
         "source":"plugin","trustStatus":"untrusted","enabled":True},
        {"key":str(tmp_path / "hooks.json")+":pre_tool_use:0:0","currentHash":"sha256:account",
         "source":"user","trustStatus":"modified","enabled":True},
        {"key":"repo/hooks.json:0","currentHash":"sha256:repo","source":"project",
         "trustStatus":"untrusted","enabled":True},
        {"key":"off","currentHash":"sha256:off","source":"plugin",
         "trustStatus":"untrusted","enabled":False},
    ]}]}
    edits, manual = edicoes_confianca_hooks(data,tmp_path)
    assert any(edit["value"] == "sha256:decl/sha256:content" for edit in edits)
    assert any(edit["value"] == "sha256:account" for edit in edits)
    assert not any(edit["value"] in ("sha256:repo","sha256:off") for edit in edits)
    assert manual > 0


WRAPPER = Path(__file__).resolve().parents[2] / "scripts" / "codex-hook-allow.py"


def _config(command, *, evento="PreToolUse", **extra):
    return {"outra_chave": {"preservada": True}, "hooks": {evento: [
        {"matcher": "Bash|exec_command", "outro_campo": True,
         "hooks": [{"type": "command", "command": command, **extra}]},
    ]}}


def _hook(config, evento="PreToolUse"):
    return config["hooks"][evento][0]["hooks"][0]


def _rodar(entrada, *args):
    return subprocess.run([sys.executable, str(WRAPPER), *args], input=entrada,
                          capture_output=True, timeout=10)


@pytest.mark.parametrize("entrada", [
    b"", b"texto\n", b"\xff\xfe", b"null", b"true", b"42", b"[]", b'"texto"',
    b'{"hookSpecificOutput":null}', b'{"hookSpecificOutput":[]}',
    b'{"hookSpecificOutput":"updatedInput"}',
    b'{"hookSpecificOutput":{"updatedInput":{}}}',
    b'{"hookSpecificOutput":{"hookEventName":"PostToolUse","updatedInput":{}}}',
    b'{"hookSpecificOutput":{"hookEventName":"PreToolUse"}}',
])
def test_saida_sem_reescrita_valida_passa_byte_a_byte(entrada):
    resultado = _rodar(entrada)
    assert resultado.returncode == 0
    assert resultado.stdout == entrada
    assert resultado.stderr == b""


@pytest.mark.parametrize("decisao", ["allow", "deny", "ask", None, False])
def test_decisao_existente_nunca_e_substituida(decisao):
    entrada = json.dumps({"hookSpecificOutput": {
        "hookEventName": "PreToolUse", "updatedInput": {"command": "x"},
        "permissionDecision": decisao,
    }}, indent=2).encode()
    assert _rodar(entrada).stdout == entrada


def test_wrapper_preserva_payload_argumentos_stderr_e_codigo_de_bloqueio(tmp_path):
    programa = tmp_path / "hook com espaço.py"
    programa.write_text(
        "import json, sys\n"
        "entrada = json.load(sys.stdin)\n"
        "assert sys.argv[1:] == ['espaço e acentuação', 'a;b', '']\n"
        "sys.stderr.buffer.write(b'aviso\\xff\\n')\n"
        "print(json.dumps({'hookSpecificOutput': {'hookEventName': 'PreToolUse', "
        "'updatedInput': entrada}}))\n"
        "sys.exit(2)\n", encoding="utf-8",
    )
    resultado = _rodar(b'{"command":"pwd; cat x"}', "--", sys.executable,
                      str(programa), "espaço e acentuação", "a;b", "")
    assert resultado.returncode == 2
    assert resultado.stderr == b"aviso\xff\n"
    especifico = json.loads(resultado.stdout)["hookSpecificOutput"]
    assert especifico["permissionDecision"] == "allow"
    assert especifico["updatedInput"] == {"command": "pwd; cat x"}


def test_wrapper_binario_inexistente_falha_visivelmente(tmp_path):
    resultado = _rodar(b"{}", "--", str(tmp_path / "inexistente"))
    assert resultado.returncode == 127
    assert "Não foi possível" in resultado.stderr.decode()


def test_json_com_substituto_unicode_isolado_continua_valido():
    entrada = b'{"hookSpecificOutput":{"hookEventName":"PreToolUse","updatedInput":{"command":"\\ud800"}}}'
    resultado = _rodar(entrada)
    assert resultado.returncode == 0
    especifico = json.loads(resultado.stdout)["hookSpecificOutput"]
    assert especifico["permissionDecision"] == "allow"
    assert especifico["updatedInput"]["command"] == "\ud800"


@pytest.mark.skipif(os.name == "nt", reason="Executa o shell POSIX")
def test_comando_normalizado_executa_via_shell_sem_esconder_bloqueio(tmp_path):
    rtk = tmp_path / "pasta com espaço" / "rtk"
    rtk.parent.mkdir()
    rtk.write_text(
        f"#!{sys.executable}\n"
        "import sys\n"
        "assert sys.argv[1:] == ['hook', 'claude', 'a;b', 'a|b', '']\n"
        "assert sys.stdin.buffer.read() == b'payload'\n"
        "sys.stdout.buffer.write(b'nao-json\\xff')\n"
        "sys.stderr.buffer.write(b'bloqueado')\n"
        "sys.exit(2)\n", encoding="utf-8",
    )
    rtk.chmod(0o700)
    config = _config(shlex.join([str(rtk), "hook", "claude", "a;b", "a|b", ""]))
    normalizado = normalizar_hooks(config, sys.executable, WRAPPER)
    resultado = subprocess.run(_hook(normalizado)["command"], shell=True, input=b"payload",
                              capture_output=True, cwd=tmp_path, timeout=10)
    assert resultado.returncode == 2
    assert resultado.stdout == b"nao-json\xff"
    assert resultado.stderr == b"bloqueado"


@pytest.mark.parametrize("command", [
    "rtk hook claude", "rtk hook claude --flag 'valor com espaço' ''",
    "'/pasta com espaço/rtk' hook claude --flag 'a;b'",
    "rtk hook claude | python3 /antigo/codex-hook-allow.py",
    "rtk hook claude --flag 'a|b' | '/bin/python3.14' '/antigo com espaço/codex-hook-allow.py'",
])
def test_normalizacao_rtk_preserva_argumentos_campos_e_e_idempotente(command):
    config = _config(command, timeout=20, statusMessage="Executando")
    normalizado = normalizar_hooks(config, "/python com espaço", WRAPPER)
    args = shlex.split(_hook(normalizado)["command"])
    assert args[:3] == ["/python com espaço", str(WRAPPER), "--"]
    assert args[4:6] == ["hook", "claude"]
    assert _hook(normalizado)["timeout"] == 20
    assert _hook(normalizado)["statusMessage"] == "Executando"
    assert normalizado["outra_chave"] == config["outra_chave"]
    assert normalizado["hooks"]["PreToolUse"][0]["matcher"] == "Bash|exec_command"
    assert _hook(config)["command"] == command
    assert normalizar_hooks(normalizado, "/python com espaço", WRAPPER) == normalizado
    if "a;b" in command:
        assert args[-1] == "a;b"
    if "a|b" in command:
        assert args[-1] == "a|b"


@pytest.mark.parametrize("command", [
    "/x/guard.sh", "echo rtk hook claude", "rtk hook other", "rtk hook claude && echo ok",
    "rtk hook claude | python3 outro.py", "rtk hook claude; echo fim",
    "rtk hook claude --flag $VAR", 'rtk hook claude --flag "$VAR"',
    "rtk hook claude > arquivo", "rtk hook claude | | python3 codex-hook-allow.py",
    "rtk hook claude *", "rtk hook claude # comentário", "rtk hook claude ~/arquivo",
    "rtk hook claude 'aspas incompletas", None,
])
def test_outros_comandos_e_shell_complexo_ficam_intactos(command):
    config = _config(command)
    assert normalizar_hooks(config, sys.executable, WRAPPER) == config


def test_timeout_somente_em_command_session_end_e_copia_profunda():
    config = _config("encerrar", evento="SessionEnd", timeout=600)
    config["hooks"]["SessionEnd"][0]["hooks"] += [
        {"type": "command", "command": "curto", "timeout": 2},
        {"type": "command", "command": "fracionado", "timeout": 3.1},
        {"type": "prompt", "prompt": "fim", "timeout": 600},
        {"type": "command", "command": "sem"},
        {"type": "command", "command": "estranho", "timeout": "600"},
        None,
    ]
    config["hooks"]["Stop"] = [{"hooks": [{"type": "command", "timeout": 600}]}]
    resultado = normalizar_hooks(config, sys.executable, WRAPPER)
    hooks = resultado["hooks"]["SessionEnd"][0]["hooks"]
    assert [h.get("timeout") for h in hooks if isinstance(h, dict)] == [3, 2, 3, 600, None, "600"]
    assert resultado["hooks"]["Stop"] == config["hooks"]["Stop"]
    resultado["outra_chave"]["preservada"] = False
    assert config["outra_chave"]["preservada"] is True
    assert _hook(config, "SessionEnd")["timeout"] == 600


@pytest.mark.parametrize("config", [{}, {"hooks": None}, {"hooks": []}, {
    "hooks": {"PreToolUse": [None, {}, {"hooks": None}, {"hooks": ["estranho"]}], "Stop": None},
}])
def test_estruturas_inesperadas_nao_quebram_normalizacao(config):
    assert normalizar_hooks(config, sys.executable, WRAPPER) == config


def test_windows_preserva_barras_aspas_espacos_e_argumentos():
    command = r'"C:\Program Files\RTK\rtk.exe" hook claude --flag "a&b" "C:\fim\\" "aspas\"internas"'
    python = r"C:\Program Files\Python\python.exe"
    wrapper = Path(r"C:\Hangar local\codex-hook-allow.py")
    resultado = normalizar_hooks(_config(command), python, wrapper, windows=True)
    novo = _hook(resultado)["command"]
    assert _split_windows(novo) == [python, str(wrapper), "--", *_split_windows(command)]
    # `& ` porque o Codex roda o hook pelo PowerShell no Windows (medido 7.6.6).
    assert novo.startswith(r'& "C:\Program Files\Python\python.exe"')
    assert '"a&b"' in novo
    assert normalizar_hooks(resultado, python, wrapper, windows=True) == resultado


def test_windows_migra_pipeline_legado():
    resultado = normalizar_hooks(_config(
        r'rtk hook claude | "C:\Python local\python.exe" "C:\Hangar\codex-hook-allow.py"'
    ), "python.exe", Path(r"C:\Hangar\codex-hook-allow.py"), windows=True)
    assert _split_windows(_hook(resultado)["command"])[2:] == ["--", "rtk", "hook", "claude"]


@pytest.mark.parametrize("atual", ["", "# Preferências\n\nMantenha isto.\n", "\n\nTexto com espaços  \n"])
def test_sem_bloco_preserva_instrucoes_pessoais(atual):
    assert remover_instrucao_de_leitura(atual) == atual


def test_remove_blocos_antigos_sem_perder_texto_pessoal():
    bloco = f"{INICIO_INSTRUCOES}\nantigo\n{FIM_INSTRUCOES}\n\n"
    atual = "Antes\n\n" + bloco + "Depois\n" + bloco
    assert remover_instrucao_de_leitura(atual) == "Antes\n\nDepois\n"


@pytest.mark.parametrize("atual", [
    INICIO_INSTRUCOES, FIM_INSTRUCOES, FIM_INSTRUCOES + INICIO_INSTRUCOES,
    INICIO_INSTRUCOES + INICIO_INSTRUCOES + FIM_INSTRUCOES,
    INICIO_INSTRUCOES + FIM_INSTRUCOES + FIM_INSTRUCOES,
])
def test_bloco_incompleto_ou_malformado_e_recusado(atual, tmp_path):
    with pytest.raises(ValueError, match="incompleto ou malformado"):
        remover_instrucao_de_leitura(atual)


@pytest.mark.skipif(shutil.which("rtk") is None, reason="RTK não instalado")
def test_rtk_real_em_comando_composto_com_home_isolada(tmp_path):
    entrada = json.dumps({"hook_event_name": "PreToolUse", "tool_name": "Bash",
                          "tool_input": {"command": "pwd; cat README.md"}, "cwd": str(tmp_path)}).encode()
    env = {**os.environ, "HOME": str(tmp_path), "USERPROFILE": str(tmp_path),
           "XDG_CONFIG_HOME": str(tmp_path / "config"), "XDG_DATA_HOME": str(tmp_path / "data"),
           "XDG_CACHE_HOME": str(tmp_path / "cache"), "CLAUDE_CONFIG_DIR": str(tmp_path / "claude")}
    rtk = shutil.which("rtk")
    resultado = subprocess.run([sys.executable, str(WRAPPER), "--", rtk, "hook", "claude"],
                              input=entrada, capture_output=True, env=env, cwd=tmp_path, timeout=10)
    assert resultado.returncode == 0, resultado.stderr
    especifico = json.loads(resultado.stdout)["hookSpecificOutput"]
    assert especifico["permissionDecision"] == "allow"
    assert "rtk" in especifico["updatedInput"]["command"]
