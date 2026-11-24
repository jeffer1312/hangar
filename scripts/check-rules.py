#!/usr/bin/env python3
"""Regras do CLAUDE.md que se conferem pelo diff e pelos Cargo.toml, sem compilar.

Uso: check-rules.py <base> <head>      (compara head com o ancestral comum)
Saída 0 = ok; 1 = regra descumprida; 2 = intervalo ilegível.
"""
import re
import subprocess
import sys
import tomllib
from pathlib import Path
from typing import NamedTuple

SERVER = "hangar-server"
TESTS = re.compile(r"\.test\.|(^|/)tests?/|/__mocks__/|(^|/)fixtures?(/|\.)|(^|[/_])tests?\.rs$|/cases\.rs$|\.nativeSetup\.|vitest\.config\.|(^|/)test[_-]")
COMMENT = re.compile(r"\s*(//|#|\*|/\*|<!--)")
KEYBOARD = re.compile(
    r"import\s+(?:type\s+)?(?:\w+\s*,\s*)?\{[^}]*\bKeyboardAvoidingView\b[^}]*\}\s*from\s*['\"]react-native['\"]")


class LineRule(NamedTuple):
    """Uma linha nova que casa com `pattern` nos arquivos de `paths`; o que já existia não conta."""
    mark: str
    title: str
    help: str
    paths: tuple[str, ...]
    pattern: re.Pattern
    allowed: re.Pattern


LINE_RULES = (
    LineRule(
        "NOME_ANTIGO", "Nome antigo (claude-pocket)",
        "O nome antigo só existe em ponte de compatibilidade: escreva com o nome novo e não leia o antigo.",
        ("*", ":!*.md"), re.compile(r"claude[-_]pocket", re.I), re.compile(r"^scripts/(test_)?check[-_]rules\.py$")),
    LineRule(
        "VIBRACAO", "Vibração fora do src/ui/haptics.ts",
        "No Expo, a vibração sai só por mobile/src/ui/haptics.ts.",
        ("mobile/*.ts", "mobile/*.tsx"), re.compile(r"['\"]expo-haptics['\"]|\bHaptics\.\w|\bVibration\.\w"),
        re.compile(r"^mobile/src/ui/haptics\.ts$")),
    LineRule(
        "ENDERECO", "Chamada com baseUrl cru",
        "Toda URL de chamada a um servidor sai por baseOf(servidor) (ou getBaseUrl do ApiEnv): baseUrl cru ignora a rede local.",
        tuple(f"{folder}/*.{ext}" for folder in ("frontend/src", "mobile/src", "mobile/app", "packages/core/src")
              for ext in ("ts", "tsx", "svelte")),
        re.compile(r"\.baseUrl\}(/(api|ws|cdp)\b|\$\{)|\.baseUrl\s*\+|new URL\([^)]*,\s*[\w.?]*\.baseUrl\s*\)"),
        re.compile(r"(?!)")),
    LineRule(
        "LANCADOR", "Sessão aberta fora do lançador",
        "Abrir sessão é de um lançador só (crates/hangar-server/src/session_open); o resto manda um SessionOpenRequest.",
        ("crates/*.rs",), re.compile(r"['\"]([^'\"\n]*\s)?new-session(?![\w-])"), re.compile(r"^crates/hangar-server/src/session_open/")),
)


def authorized(base: str, head: str, mark: str) -> bool:
    # O motivo de verdade: o "<motivo>" copiado da ajuda não vale.
    pattern = rf"(^|[^A-Za-z_])HANGAR_{mark}_AUTORIZADO:[ \t]+(?!<)\S"
    return re.search(pattern, git("log", "--no-merges", "--format=%B", f"{base}..{head}"), re.M) is not None


def added_lines(base: str, head: str, *paths: str, renames: bool = True) -> list[tuple[str, str]]:
    """(nome no head, texto) de cada linha que o diff acrescenta."""
    found, name, header = [], "", False
    for line in git("-c", "core.quotePath=false", "diff", "--find-renames" if renames else "--no-renames", "--no-ext-diff",
                    "--no-color", "--src-prefix=a/", "--dst-prefix=b/", "-U0", f"{base}...{head}", "--", *paths).splitlines():
        if line.startswith("diff --git "):
            name, header = "", True
        elif line.startswith("@@"):
            header = False
        elif header:
            # Só no cabeçalho: uma linha de conteúdo "++ x" vira "+++ x" no diff.
            if line.startswith("+++ b/"):
                name = line[6:].rstrip("\t")
        elif line.startswith("+"):
            found.append((name, line[1:]))
    return found


def git(*args: str) -> str:
    done = subprocess.run(["git", *args], capture_output=True, text=True, encoding="utf-8", errors="replace")
    if done.returncode:
        print(f"git {' '.join(args)}: {done.stderr.strip()}", file=sys.stderr)
        raise SystemExit(2)
    return done.stdout


def new_python(base: str, head: str) -> list[str]:
    """Linhas que o backend ganhou: nada nasce no Python sem autorização do dono."""
    if authorized(base, head, "PYTHON"):
        return []
    # Sem rename: arquivo movido para dentro conta inteiro.
    return [f"{name}: {line.strip()[:120]}" for name, line in added_lines(base, head, "backend/app", renames=False)
            if line.strip() and not line.lstrip().startswith("#")]


def matches(text: str, rule: LineRule) -> list[tuple[str, str]]:
    """(identidade, trecho) de cada caso: as linhas inteiras que o casamento ocupa, sem espaço e com uma aspa só."""
    found = []
    for match in rule.pattern.finditer(text):
        start, end = text.rfind("\n", 0, match.start()) + 1, text.find("\n", match.end())
        lines = text[start:end if end >= 0 else len(text)]
        if not COMMENT.match(lines):
            found.append((re.sub(r"\s+", "", re.sub(r"[\"`]", "'", lines)), " ".join(lines.split())))
    return found


def new_lines(base: str, head: str, rule: LineRule) -> list[str]:
    """O arquivo inteiro, antes e depois: só é velho o mesmo trecho que já existia, mesmo reformatado ou movido
    entre arquivos do PR. Casa também a chamada que ocupa várias linhas."""
    start = git("merge-base", base, head).strip()
    old: dict[str, int] = {}
    fresh: list[tuple[str, str, str]] = []
    for status, before, after in changed(base, head, *rule.paths):
        for key, _ in matches(show(start, before), rule):
            old[key] = old.get(key, 0) + 1
        if status != "D" and not TESTS.search(after) and not rule.allowed.search(after):
            fresh += [(after, key, line) for key, line in matches(show(head, after), rule)]
    found = []
    for name, key, line in fresh:
        if old.get(key, 0) > 0:
            old[key] -= 1
        else:
            found.append(f"{name}: {line[:120]}")
    return [] if not found or authorized(base, head, rule.mark) else found


def changed(base: str, head: str, *paths: str) -> list[tuple[str, str, str]]:
    """(estado, caminho antes, caminho depois) de cada arquivo do diff, com renomeação."""
    items = git("diff", "--find-renames", "--name-status", "-z", f"{base}...{head}", "--", *paths).split("\0")
    found, i = [], 0
    while i < len(items) and items[i]:
        status = items[i][0]
        if status in "RC":
            found.append((status, items[i + 1], items[i + 2]))
            i += 3
        else:
            found.append((status, items[i + 1], items[i + 1]))
            i += 2
    return found


def show(rev: str, path: str) -> str:
    done = subprocess.run(["git", "show", f"{rev}:{path}"], capture_output=True, text=True, encoding="utf-8", errors="replace")
    return done.stdout if done.returncode == 0 and "\0" not in done.stdout else ""


def changed_files(base: str, head: str, status: str, *paths: str) -> list[str]:
    return git("-c", "core.quotePath=false", "diff", "--find-renames", "--name-only", f"--diff-filter={status}",
               f"{base}...{head}", "--", *paths).splitlines()


def new_web_screens(base: str, head: str) -> list[str]:
    """A web fica com o que já tem: componente novo é recurso novo até o dono dizer o contrário."""
    found = [name for name in changed_files(base, head, "A", "frontend/src/*.svelte") if not TESTS.search(name)]
    return [] if not found or authorized(base, head, "WEB") else found


def raw_keyboard(base: str, head: str) -> list[str]:
    """O import pode ocupar várias linhas: vale o arquivo inteiro, e só o que passou a importar conta."""
    start = git("merge-base", base, head).strip()
    found = [after for status, before, after in changed(base, head, "mobile/*.ts", "mobile/*.tsx")
             if status != "D" and not TESTS.search(after) and KEYBOARD.search(show(head, after))
             and not KEYBOARD.search(show(start, before))]
    return [] if not found or authorized(base, head, "TECLADO") else found


def server_dependents() -> list[str]:
    """Nenhum crate depende do hangar-server: o que ele precisa desce para o hangar-core."""
    root = Path(git("rev-parse", "--show-toplevel").strip())
    found = []
    for manifest in [*sorted(root.glob("crates/*/Cargo.toml")), root / "desktop-native" / "Cargo.toml"]:
        if not manifest.is_file() or manifest.parent.name == SERVER:
            continue
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        for scope in [data, *data.get("target", {}).values()]:
            for kind in ("dependencies", "dev-dependencies", "build-dependencies"):
                for name, spec in scope.get(kind, {}).items():
                    if name == SERVER or (isinstance(spec, dict) and spec.get("package") == SERVER):
                        found.append(f"{manifest.relative_to(root).as_posix()} ({kind})")
    return found


def with_mark(mark: str) -> str:
    return f" Com autorização do dono, ponha 'HANGAR_{mark}_AUTORIZADO: <motivo>' na mensagem de um commit."


def main() -> int:
    if len(sys.argv) != 3:
        print("uso: check-rules.py <base> <head>", file=sys.stderr)
        return 2
    base, head = sys.argv[1:]
    broken = [
        ("Linha nova no backend Python",
         "Nada nasce no Python: o lugar é o crates/." + with_mark("PYTHON"), new_python(base, head)),
        ("Crate dependendo do hangar-server",
         "O que o módulo precisa desce para o hangar-core, ou vira trait que o servidor implementa.", server_dependents()),
        ("Tela nova na web",
         "A web (frontend/) fica com o que já tem; recurso novo vai para desktop-native/ e mobile/." + with_mark("WEB"), new_web_screens(base, head)),
        ("KeyboardAvoidingView do react-native",
         "Use o do react-native-keyboard-controller, com automaticOffset." + with_mark("TECLADO"), raw_keyboard(base, head)),
        *((rule.title, rule.help + with_mark(rule.mark), new_lines(base, head, rule)) for rule in LINE_RULES),
    ]
    failed = False
    for title, help_text, found in broken:
        if not found:
            print(f"ok: {title}: nenhuma")
            continue
        failed = True
        print(f"::error title={title}::{help_text}")
        for item in found[:20]:
            print(f"  {item}")
        if len(found) > 20:
            print(f"  … e mais {len(found) - 20}")
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
