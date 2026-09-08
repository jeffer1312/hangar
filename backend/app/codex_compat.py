"""Transformações puras para importar hooks e instruções do Claude no Codex."""

from copy import deepcopy
from pathlib import Path, PureWindowsPath
import re
import shlex
import subprocess


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
    return args


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
    tokens = []
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
