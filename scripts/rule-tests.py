#!/usr/bin/env python3
"""Roda os testes que travam uma regra do CLAUDE.md, por área, e confere que cada um rodou de fato.

O filtro do cargo e o do vitest passam verdes com zero testes: um teste renomeado tiraria a trava do
portão (gate.yml) sem ninguém ver. Por isso a lista é por nome, e nome que não rodou é falha.

Uso: rule-tests.py <server|native|mobile|frontend>
"""
import json
import re
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent

# Abertura única: todo padrão é do servidor, e todo campo do pedido sai do CLI e do MCP.
SERVER = [
    "session_open::tests::every_open_field_is_said_by_the_cli_and_the_mcp_or_is_only_of_the_server",
    "session_open::tests::a_field_with_a_server_default_keeps_the_explicit_value",
    "session_open::tests::code_mode_default_of_the_server_fills_an_omitted_request",
    "mcp::tools::tests::new_session_says_every_open_field",
    "new_says_every_open_field",
    "tests::the_scheduled_launch_only_keeps_open_fields",
]
NATIVE = [
    "app::create::default_tests::the_open_body_does_not_depend_on_what_this_computer_saved",
    "app::create::scheduled_launch::tests::a_new_template_does_not_freeze_what_the_screen_showed",
    "app::create::scheduled_launch::tests::switching_provider_on_a_new_template_keeps_only_the_provider",
]
# Arquivo → títulos; lista vazia é o arquivo inteiro.
MOBILE = {
    "src/features/create/CreateSessionSheet.test.tsx": [
        "modelo e esforço lembrados no aparelho não vão no pedido sem padrão do servidor",
        "modo de execução vem do servidor e só vai no pedido quando a pessoa escolhe",
        "conta Codex lembrada no aparelho não substitui a padrão do servidor",
    ],
    "src/theme/fontScaleGuard.test.ts": [],
    "src/theme/layoutScaleGuard.test.ts": [],
}
# Texto de interface vem das mensagens, e pt e en têm as mesmas chaves.
FRONTEND = {"src/lib/i18nGuard.test.ts": []}


def cargo(folder: str, args: list[str], names: list[str]) -> list[str]:
    command = ["cargo", "test", "--locked", "--no-fail-fast", *args, "--", "--exact", "--test-threads=1", *names]
    print("+", " ".join(command), flush=True)
    child = subprocess.Popen(command, cwd=ROOT / folder, stdout=subprocess.PIPE, text=True, encoding="utf-8", errors="replace")
    ran: dict[str, str] = {}
    for line in child.stdout:
        print(line, end="", flush=True)
        match = re.match(r"test (\S+) \.\.\. (\S+)", line)
        if match:
            ran[match[1]] = match[2]
    code = child.wait()
    problems = [f"{name}: {ran.get(name, 'não rodou')}" for name in names if ran.get(name) != "ok"]
    if code and not problems:
        problems.append(f"cargo test saiu com {code}")
    return problems


def vitest(folder: str, files: dict[str, list[str]]) -> list[str]:
    problems = []
    for file, titles in files.items():
        with tempfile.TemporaryDirectory() as tmp:
            out = Path(tmp) / "out.json"
            command = ["npx", "vitest", "run", file, "--reporter=default", "--reporter=json", f"--outputFile.json={out}"]
            if titles:
                command += ["-t", "|".join(re.escape(t) for t in titles)]
            print("+", " ".join(command), flush=True)
            code = subprocess.run(command, cwd=ROOT / folder).returncode
            results = json.loads(out.read_text(encoding="utf-8"))["testResults"] if out.is_file() else []
        # Título repetido em outro describe: o pior resultado vale.
        status: dict[str, str] = {}
        for result in (a for suite in results for a in suite["assertionResults"]):
            if status.get(result["title"], "passed") == "passed":
                status[result["title"]] = result["status"]
        passed = [t for t, s in status.items() if s == "passed"]
        problems += [f"{file}: {t}: {status.get(t, 'não rodou')}" for t in titles if t not in passed]
        if not titles:
            problems += [f"{file}: {t}: {s}" for t, s in status.items() if s != "passed"]
        if not passed:
            problems.append(f"{file}: nenhum teste passou")
        elif code:
            problems.append(f"{file}: vitest saiu com {code}")
    return problems


AREAS = {
    # `--workspace --lib`: as mesmas features do cache que o server-jobs.yml grava.
    "server": lambda: cargo("crates", ["--workspace", "--lib", "--test", "open_fields"], SERVER),
    "native": lambda: cargo("desktop-native", [], NATIVE),
    "mobile": lambda: vitest("mobile", MOBILE),
    "frontend": lambda: vitest("frontend", FRONTEND),
}


def main() -> int:
    if len(sys.argv) != 2 or sys.argv[1] not in AREAS:
        print(f"uso: rule-tests.py <{'|'.join(AREAS)}>", file=sys.stderr)
        return 2
    problems = AREAS[sys.argv[1]]()
    for problem in problems:
        print(f"::error title=Regra descumprida::{problem}")
    return 1 if problems else 0


if __name__ == "__main__":
    raise SystemExit(main())
