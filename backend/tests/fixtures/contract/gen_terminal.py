"""Golden do pane e do StateMonitor Python, sem terminal ou sidecar real.

Uso, de backend/: uv run python tests/fixtures/contract/gen_terminal.py
"""
import asyncio
import inspect
import json
import sys
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import patch

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parents[2]))
from app import state, terminal_observer
from app.loop import LoopLink
from app.preview import extract_assistant_text


def analyze(pane):
    status, label, question, options = state.classify(pane)
    menu = state.menu_codex(pane)
    return dict(state=status, label=label, question=question, options=options,
                spinner=state._live_spinner(pane), status_line=state.status_line(pane),
                overlay=state.is_overlay(pane), login=state.is_login(pane),
                limit_reset=state.rate_limit_reset(pane),
                preview=extract_assistant_text(pane, "claude"),
                codex_menu=None if menu is None else dict(question=menu[0], options=menu[1]))


class Finished(Exception):
    pass


async def reference_sequence(frames):
    """Executa o reducer original; observa os locais na próxima aquisição de pane."""
    outputs, current = [], {}
    index = 0
    monitor = state.StateMonitor("fixture", sid_get=lambda: "fixture", poll=0, provider=None)

    async def capture(*args):
        nonlocal index, current
        caller = inspect.currentframe().f_back
        if index:
            values = caller.f_locals
            result = analyze(current["pane"])
            for name in ("state", "label", "question", "options", "overlay", "login", "limit_reset"):
                result[name] = values[name]
            result["status_line"] = values["status"]
            memory = {name: values[name] for name in
                      ("prev_spinner", "frozen", "no_spinner", "held_state", "held_label")}
            outputs.append(dict(analysis=result, memory=memory))
        if index == len(frames):
            raise Finished
        current = frames[index]
        index += 1
        monitor.hook_grace = current.get("facts", {}).get("hook_grace", 8)
        return current["pane"]

    def fact(name):
        return current.get("facts", {}).get(name)

    def open_question(*args):
        q = fact("open_question")
        return None if q is None else SimpleNamespace(questions=[SimpleNamespace(
            question=q.get("question"), options=[SimpleNamespace(label=o) for o in q["options"]])])

    async def no_sleep(*args):
        pass

    with patch.object(state, "shared_capture", capture), \
         patch.object(terminal_observer, "_config", None), \
         patch.object(state, "pergunta_aberta", open_question), \
         patch.object(state.plugin_bridge, "pergunta_pendente", lambda *a: fact("plugin_question")), \
         patch.object(state.plugin_bridge, "estado_recente", lambda *a: None if fact("plugin_state") is None else (fact("plugin_state"), 0)), \
         patch.object(state.plugin_bridge, "vivo", lambda *a: False), \
         patch.object(state.hook_state, "get_state", lambda *a: None if fact("hook_state") is None else (fact("hook_state"), 0)), \
         patch.object(state.hook_state, "shells", lambda *a: []), \
         patch.object(state, "_sidecar_status", lambda *a: fact("status_line")), \
         patch.object(LoopLink, "get", lambda *a: None), \
         patch.object(state.asyncio, "sleep", no_sleep):
        try:
            async for _ in monitor.stream():
                pass
        except Finished:
            pass
    return outputs


def main():
    rows = [dict(name=p.name, pane=p.read_text(encoding="utf-8"))
            for p in sorted(HERE.parent.glob("pane_*.txt"))]
    synthetic = {
        "empty": "",
        "prose_utf8": "❯ teste\n● Olá, ação 🐍\n  continuação 漢字\n────────────\n❯\n────────────\nmodelo truncado",
        "tools_keep_prose": "● Resposta\n  texto\n● Bash(ls)\n⎿ output\n● Reading 4 files…\n✻ Thinking…",
        "plugin_reset": "❯ teste\n● Resposta anterior\n● ecc: hooks.json: unknown keys\n────────────",
        "banner": "Claude Code v2\n● aviso\n────────────",
        "subagent": "● Resposta\n  continuação\n● Subagent reviewer\n\n └ conferindo\n────────────",
        "agent_finished": '● Resposta\n● Agent "reviewer" finished · 9s\nSearched, ran 2 shell commands',
        "mcp": "● Resposta\nCalling chrome-devtools…\nfora",
        "mcp_prose": "● Calling this an edge case, veja.\n  continuação",
        "activity": "● Resposta\n  resumo ran 2 shell commands\nfora",
        "ascii_spinner": "● Resposta\n* Thinking…\nfora",
        "todo": "● Resposta\n○ Todos (1/2)\nfora",
        "draft": "✻ Worked for 1s\n────────────\n❯ 1. pode editar 2. mostre a data\n────────────",
        "preview_numbered": "☐ Escolha\nQual?\n❯ 1. ação 🐍          ┌───────────\n  2. outra             │ 1. exemplo 2. segundo\nEnter to select",
        "login": "Choose the text style\n\n\n",
        "quoted_login": "● /oauth/authorize\n────────────\n❯\n────────────\n⏵⏵ bypass",
        "overlay_padded": "painel\nEsc to cancel\n" + "\n" * 20,
        "overlay_old": "Esc to cancel\n" + "texto\n" * 12,
        "limit_old": "Usage limit reached resets 9:10pm\n" + "texto\n" * 12,
        "codex_wrap": "Aprovar?\n› 1. Opção ação\n     continua 🐍\n  2. Outra\nPress enter to confirm\n\n",
        "codex_invalid": "Aprovar?\n› 1. Um\n  3. Três\nPress enter to confirm",
        "subagent_prose": "● Subagent reviewer pode ajudar\n  explique",
        "prose_box": "● Texto\n╭────────╮\n│ exemplo │\n╰────────╯",
        "unicode_columns": "Título\n  ❯ Sim 🐍\n    Não 漢字\nEsc to cancel",
        "unicode_numbering": "Aprovar?\n› ١. Um\n  ٢. Dois\nPress enter to confirm",
        "unicode_word_boundary": "● Running\u0301 palavras\n  continuação",
        "unicode_plugin_word": "● ecc\u0301: hooks.json: unknown keys\n  continuação",
        "combining_running": "● Running\u0345 palavras\n  continuação",
        "combining_calling": "● Calling\u05b0 serviço…\n  continuação",
        "combining_finished": '● Resposta\n● Agent "worker" finished\u0345\n  atividade',
        "combining_summary": "● Resposta\n  \u05b0ran 2 shell commands\n  fora",
        "login_dotless": "Select logın method",
        "login_dotted": "Select logİn method",
        "limit_dotless": "Usage lımıt reached · continuing automatically at 9:10pm",
        "limit_dotted": "Usage lİmİt reached · contİnuİng automatİcally at 9:10pm",
        "login_long_s": "ſelect login method",
        "login_ordinary": "Select LOGIN method",
        "information_whitespace": "● texto\n\x1f\x1f\n────────────\n\x1f",
        "line_separators": "● texto\rcontinuação\vlinha\fquadro\x1cregistro\x1dgrupo\x1earquivo\x85próxima\u2028separador\u2029parágrafo",
    }
    rows += [dict(name=name, pane=pane) for name, pane in synthetic.items()]
    for row in rows:
        row["expected"] = analyze(row["pane"])
    frame = lambda pane, **facts: dict(pane=pane, facts=facts)
    spinner = "✻ Thinking…\n────────────\n❯\n────────────"
    plain = "● Resposta\n────────────\n❯\n────────────"
    menu = synthetic["preview_numbered"]
    sequences = {
        "frozen_four": [frame(spinner)] * 4,
        "missing_four": [frame(spinner)] + [frame(plain)] * 4,
        "hook_grace_eight": [frame(plain, hook_state="working")] * 10,
        "hook_no_grace": [frame(plain, hook_state="working", hook_grace=None)] * 10,
        "menu_immediate": [frame(spinner), frame(menu, plugin_state="working", hook_state="working")],
        "plugin_idle_animation": [frame(spinner), frame(spinner.replace("✻", "✽"), plugin_state="idle")],
        "hook_idle_animation": [frame(spinner), frame(spinner.replace("✻", "✽"), hook_state="idle"), frame(spinner.replace("✻", "✽"), hook_state="idle")],
        "plugin_then_hook": [frame(plain, plugin_state="idle", hook_state="working")] * 10,
        "plugin_working_resets": [frame(plain, plugin_state="working")] * 10,
        "statusline_truthy": [frame(spinner, status_line="modelo inteiro 🐍"), frame(spinner, status_line="")],
        "offscreen_question": [frame(spinner, open_question=dict(question="Qual?", options=["Um", "Dois"]))],
        "pane_question_wins": [frame(menu, open_question=dict(question="Outro?", options=["X", "Y"]), plugin_question=dict(id="perm:a", tool="Bash", resumo="ls"))],
        "plugin_permission": [frame(spinner, plugin_question=dict(id="perm:a", tool="Bash", resumo="ls"))],
        "plugin_question": [frame(plain, plugin_question=dict(id="q", questions=[dict(question="Qual?", options=[dict(label="Um"), dict(label="Dois")])]))],
        "plugin_empty_question": [frame(plain, plugin_question=dict(id="q", questions=[]))],
        "non_animated_change": [frame(spinner), frame("✽ Worked for 3s", hook_state="idle")],
    }
    # A mesma coleção conserva o formato simples das fixtures estáticas.
    rows += [dict(name=name, sequence=frames, expected_sequence=asyncio.run(reference_sequence(frames)))
             for name, frames in sequences.items()]
    target = HERE / "golden" / "terminal.json"
    target.write_text(json.dumps(rows, ensure_ascii=False, indent=2) + "\n", encoding="utf-8")
    print(f"{len(rows)} casos gravados em {target}")


if __name__ == "__main__":
    main()
