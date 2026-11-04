"""Transformações e compatibilidades de hooks do Claude no Codex."""

from copy import deepcopy
import json
import logging
import os
from pathlib import Path, PureWindowsPath
import re
import shlex
import shutil
import subprocess
from typing import Callable

from app import install_root
from app.codex_arquivos import gravar, json_bytes, json_obj, ler, transformar


_REPO = install_root.root()  # o `current` com o pacote: a cópia do wrapper sai da versão em uso
_log = logging.getLogger(__name__)


INICIO_INSTRUCOES = "<!-- hangar:codex-instrucoes:start -->"
FIM_INSTRUCOES = "<!-- hangar:codex-instrucoes:end -->"


def _partes_shell(command: str, *, windows: bool) -> list[str] | None:
    """Separa apenas pipes literais; recusa expansões e outras operações de shell."""
    partes, inicio = [], 0
    aspas = None
    i = 0
    while i < len(command):
        char = command[i]
        if char == "\\" and not windows and aspas != "'":
            i += 2
            continue
        if windows and char == "\\":
            j = i
            while j < len(command) and command[j] == "\\":
                j += 1
            if j == len(command) or command[j] != '"':
                i = j
                continue
            if (j - i) % 2:
                i = j + 1
                continue
            i = j
            char = command[i]
        if char in (('"',) if windows else ("'", '"')):
            if aspas == char:
                aspas = None
            elif aspas is None:
                aspas = char
        elif ((windows and char in "%!")
              or (not windows and aspas != "'" and char in "$`")):
            return None
        elif aspas is None:
            if char == "|":
                partes.append(command[inicio:i].strip())
                inicio = i + 1
            elif char in ";&<>\r\n()" or (windows and char == "^"):
                return None
            elif not windows and char in "*?[]{}~#":
                return None
        i += 1
    if aspas is not None:
        return None
    partes.append(command[inicio:].strip())
    return partes


def _split_windows(command: str) -> list[str]:
    """Lê argumentos com as regras de aspas e barras da linha de comando Windows."""
    args, atual = [], []
    aspas = iniciado = False
    i = 0
    while i < len(command):
        char = command[i]
        if char in " \t" and not aspas:
            if iniciado:
                args.append("".join(atual))
                atual, iniciado = [], False
            i += 1
            continue
        iniciado = True
        if char == "\\":
            j = i
            while j < len(command) and command[j] == "\\":
                j += 1
            barras = j - i
            if j < len(command) and command[j] == '"':
                atual.extend("\\" * (barras // 2))
                if barras % 2:
                    atual.append('"')
                else:
                    aspas = not aspas
                i = j + 1
            else:
                atual.extend("\\" * barras)
                i = j
        elif char == '"':
            aspas = not aspas
            i += 1
        else:
            atual.append(char)
            i += 1
    if aspas:
        raise ValueError("Aspas incompletas no comando Windows")
    if iniciado:
        args.append("".join(atual))
    # O `&` de invocacao do PowerShell nao e argumento.
    return args[1:] if args[:1] == ["&"] else args


def _nome_binario(value: str) -> str:
    return PureWindowsPath(value).name.lower()


def _argv_rtk(command: str, *, windows: bool) -> list[str] | None:
    partes = _partes_shell(command, windows=windows)
    if not partes or len(partes) > 2:
        return None
    split = _split_windows if windows else shlex.split
    try:
        args = split(partes[0])
        if len(args) < 3 or _nome_binario(args[0]) not in {"rtk", "rtk.exe"}:
            return None
        if args[1:3] != ["hook", "claude"]:
            return None
        if len(partes) == 2:
            filtro = split(partes[1])
            if (len(filtro) != 2
                    or not re.fullmatch(r"python(?:\d+(?:\.\d+)*)?(?:\.exe)?", _nome_binario(filtro[0]))
                    or _nome_binario(filtro[1]) != "codex-hook-allow.py"):
                return None
        return args
    except ValueError:
        return None


def wrapper_instalado(config: dict, *, windows: bool = False) -> tuple[str, str] | None:
    """(python, wrapper) do rtk já embrulhado no hooks.json, se houver.

    O comando gravado carrega o interpretador e o checkout de quem normalizou; reescrever com os do
    processo atual muda o hook e, no Codex, hook alterado é hook não aprovado. Quem já está lá
    manda: a próxima normalização reusa o par em vez de `sys.executable` + este checkout."""
    split = _split_windows if windows else shlex.split
    for grupo in (config.get("hooks") or {}).get("PreToolUse", []) if isinstance(config.get("hooks"), dict) else []:
        for hook in grupo.get("hooks", []) if isinstance(grupo, dict) else []:
            command = hook.get("command") if isinstance(hook, dict) else None
            if not isinstance(command, str):
                continue
            try:
                args = split(command)
            except ValueError:
                continue
            if (len(args) >= 6 and args[2:6] == ["--", "rtk", "hook", "claude"]
                    and _nome_binario(args[1]) == "codex-hook-allow.py"):
                return args[0], args[1]
    return None


def _comando(args: list[str], *, windows: bool) -> str:
    if not windows:
        return shlex.join(args)
    # list2cmdline trata barras finais/aspas; envolver os argumentos simples
    # também protege metacaracteres como & e | do cmd.exe.
    # `& ` na frente: o Codex roda o hook pelo PowerShell (medido 7.6.6), onde uma string entre
    # aspas no inicio da linha nao executa nada (UnexpectedToken) — o hook saia com 1 sempre.
    tokens = ["&"]
    for arg in args:
        token = subprocess.list2cmdline([arg])
        if not token.startswith('"'):
            barras_finais = len(token) - len(token.rstrip("\\"))
            token = '"' + token + "\\" * barras_finais + '"'
        tokens.append(token)
    return " ".join(tokens)


def normalizar_hooks(config: dict, python_bin: str, wrapper: Path, *, windows: bool = False) -> dict:
    """Retorna cópia normalizada do documento, sem tocar configurações em disco."""
    novo = deepcopy(config)
    eventos = novo.get("hooks")
    if not isinstance(eventos, dict):
        return novo
    for evento, grupos in eventos.items():
        if not isinstance(grupos, list):
            continue
        for grupo in grupos:
            if not isinstance(grupo, dict) or not isinstance(grupo.get("hooks"), list):
                continue
            for hook in grupo["hooks"]:
                if not isinstance(hook, dict) or hook.get("type") != "command":
                    continue
                timeout = hook.get("timeout")
                if evento == "SessionEnd" and isinstance(timeout, (int, float)) and timeout > 3:
                    hook["timeout"] = 3
                command = hook.get("command")
                if evento == "PreToolUse" and isinstance(command, str):
                    args = _argv_rtk(command, windows=windows)
                    if args is not None:
                        hook["command"] = _comando([python_bin, str(wrapper), "--", *args], windows=windows)
    return novo


def normalizar_security_guidance(config: dict, python_bin: str, wrapper: Path, *, windows: bool = False) -> dict:
    """Somente o plugin confirmado pelo chamador emite este protocolo de telemetria."""
    novo = deepcopy(config)
    split = _split_windows if windows else shlex.split
    for grupos in novo.get("hooks", {}).values():
        for grupo in grupos:
            for hook in grupo.get("hooks", []):
                command = hook.get("command")
                if hook.get("type") != "command" or not isinstance(command, str):
                    continue
                try:
                    args = split(command)
                except ValueError:
                    args = []
                if len(args) == 4 and _nome_binario(args[1]) == wrapper.name and args[2] == "--":
                    continue
                hook["command"] = _comando([python_bin, str(wrapper), "--", command], windows=windows)
    return novo


def edicoes_confianca_hooks(dados: dict, codex_home: Path, *, recarregar: bool = False) -> tuple[list[dict], int]:
    entries = dados.get("data") if isinstance(dados,dict) else None
    if not isinstance(entries, list):
        raise ValueError("Lista de hooks inválida")
    prefixes = tuple(str(path / "hooks.json").replace("\\", "/")+":"
                     for path in (codex_home, Path.home() / ".codex"))
    edits, manual = [], 0
    for entry in entries:
        if not isinstance(entry, dict) or entry.get("errors") or entry.get("warnings"):
            raise ValueError("O Codex não conseguiu carregar todos os hooks")
        hooks = entry.get("hooks", [])
        if not isinstance(hooks, list):
            raise ValueError("Lista de hooks inválida")
        for hook in hooks:
            if not isinstance(hook, dict):
                raise ValueError("Hook inválido")
            status = hook.get("trustStatus")
            pending = status in ("untrusted", "modified")
            if hook.get("enabled") is not True or not (pending or recarregar and status == "trusted"):
                continue
            key, current = hook.get("key"), hook.get("currentHash")
            if not isinstance(key, str) or not isinstance(current, str):
                raise ValueError("Identidade de hook inválida")
            synced = hook.get("source") in ("user", "plugin") or key.replace("\\", "/").startswith(prefixes)
            if synced:
                path = ".".join(json.dumps(part) for part in ("hooks", "state", key, "trusted_hash"))
                edits.append({"keyPath":path,"value":current,"mergeStrategy":"replace"})
            elif pending:
                manual += 1
    return edits, manual


def _preparar_runtime_ecc(raiz: Path, backups: Path) -> bool:
    package = json_obj(raiz / "package.json")
    dependencies = package.get("dependencies", {})
    if not dependencies:
        return False
    if not isinstance(dependencies, dict):
        raise ValueError("Dependências do ECC inválidas")
    locked = json_obj(raiz / "package-lock.json").get("packages", {})
    expected = {}
    for name, declared in dependencies.items():
        if not isinstance(name, str) or not re.fullmatch(r"(?:@[a-z0-9._-]+/)?[a-z0-9._-]+", name):
            raise ValueError("Nome de dependência do ECC inválido")
        version = locked.get(f"node_modules/{name}", {}).get("version", declared)
        if not isinstance(version, str) or not re.fullmatch(r"\d+\.\d+\.\d+(?:[-+][0-9A-Za-z.-]+)?", version):
            raise ValueError("Dependência do ECC sem versão fixada")
        expected[name] = version
    node, npm = shutil.which("node"), shutil.which("npm")
    if not node or not npm:
        raise RuntimeError("O ECC precisa de Node e npm para concluir o preparo")
    probe = "const r=require('module').createRequire(require('path').join(process.argv[1],'package.json'));for(const n of JSON.parse(process.argv[2]))r(n)"

    def healthy():
        if any(json_obj(raiz / "node_modules" / name / "package.json").get("version") != version
               for name, version in expected.items()):
            return False
        try:
            return subprocess.run([node,"-e",probe,str(raiz),json.dumps(list(expected))],
                                  capture_output=True,timeout=15).returncode == 0
        except subprocess.TimeoutExpired:
            return False

    if healthy():
        return False
    candidates = [Path(npm).resolve(), Path(npm).parent / "node_modules/npm/bin/npm-cli.js"]
    cli = next((path for path in candidates if path.is_file() and path.suffix in (".js", ".cjs", ".mjs")), None)
    if cli is None:
        raise RuntimeError("O executável JavaScript do npm não foi encontrado")
    try:
        result = subprocess.run([node,str(cli),"install","--prefix",str(raiz),"--omit=dev",
                                 "--ignore-scripts","--no-bin-links","--no-save","--no-audit","--no-fund"],
                                capture_output=True,stdin=subprocess.DEVNULL,timeout=120,cwd=raiz)
    except subprocess.TimeoutExpired as exc:
        result = subprocess.CompletedProcess(exc.cmd,1,stdout=exc.stdout or b"",
                                             stderr=(exc.stderr or b"")+b"\nnpm deadline exceeded")
    if result.returncode or not healthy():
        diagnostic = backups / "ecc-runtime-npm.log"
        gravar(diagnostic,result.stderr + result.stdout,ler(diagnostic))
        _log.warning("O preparo das dependências do ECC falhou; diagnóstico local: %s", diagnostic)
        raise RuntimeError("As dependências do ECC não foram confirmadas")
    return True


def adaptar_hooks_plugin(
    raiz: Path,
    codex_home: Path,
    backups: Path,
    *,
    normalizar: Callable[[dict], dict],
    ao_alterar: Callable[[], None],
) -> None:
    """Aplica compatibilidades nos hooks de um plugin já instalado no cache.

    A decisão de confiança pertence ao chamador: `ao_alterar` apenas informa que o conteúdo
    mudou, sem copiar aprovações nem marcar o hook como confiável.
    """
    cache = (codex_home / "plugins" / "cache").resolve()
    raiz = raiz.resolve()
    if not raiz.is_relative_to(cache):
        raise ValueError("Plugin fora do cache do Codex")
    paths = {raiz / "hooks" / "hooks.json", raiz / "hooks.json"}
    security_guidance = False
    ecc = False
    for rel in (".codex-plugin/plugin.json", ".claude-plugin/plugin.json"):
        manifest = raiz / rel
        if not manifest.is_file():
            continue
        dados = json_obj(manifest)
        security_guidance |= dados.get("name") == "security-guidance"
        ecc |= dados.get("name") in ("ecc", "everything-claude-code")
        declaradas = dados.get("hooks")
        if isinstance(declaradas, dict):
            paths.add(manifest)
            continue
        refs = [declaradas] if isinstance(declaradas, str) else declaradas or []
        if not isinstance(refs, list) or any(not isinstance(ref, str) for ref in refs):
            raise ValueError("Referências de hooks inválidas no plugin")
        paths.update(raiz / ref for ref in refs)
    wrapper_json = codex_home / ".hangar-hooks" / "codex-hook-json.py"
    if security_guidance:
        wrapper_source = _REPO / "scripts" / "codex-hook-json.py"
        gravar(wrapper_json, wrapper_source.read_bytes(), ler(wrapper_json), backups)
    for path in sorted(paths):
        if not path.is_file():
            continue
        if not path.resolve().is_relative_to(raiz):
            raise ValueError("Hooks fora do plugin gerenciado")

        def converter(raw):
            data = json.loads(raw)
            result = normalizar(data)
            if security_guidance:
                result = normalizar_security_guidance(
                    result, install_root.interpreter(), wrapper_json, windows=os.name == "nt")
            return raw if data == result else json_bytes(result)

        if transformar(path, converter, backups):
            ao_alterar()
    if ecc and _preparar_runtime_ecc(raiz, backups):
        ao_alterar()


def remover_instrucao_de_leitura(atual: str) -> str:
    """Remove a antiga ordem de leitura, preservando as instruções pessoais."""
    restante = atual
    while INICIO_INSTRUCOES in restante or FIM_INSTRUCOES in restante:
        inicio = restante.find(INICIO_INSTRUCOES)
        fim = restante.find(FIM_INSTRUCOES)
        proximo = restante.find(INICIO_INSTRUCOES, inicio + len(INICIO_INSTRUCOES))
        if inicio < 0 or fim < inicio or (proximo >= 0 and proximo < fim):
            raise ValueError("Bloco de instruções do Hangar incompleto ou malformado")
        fim += len(FIM_INSTRUCOES)
        # Remove somente o separador que o próprio bloco gerenciado acrescenta.
        if restante[fim:fim + 2] == "\n\n":
            fim += 2
        restante = restante[:inicio] + restante[fim:]
    return restante
