# backend/tests/fixtures/contract/gen_api_samples.py
"""Amostras JSON dos formatos da conversa, geradas pelos modelos reais de app/models.py.

O crate crates/hangar-api lê cada arquivo, escreve de volta e compara: campo que mudar aqui e não
lá quebra o teste de ida e volta. Depois de mexer em ChatEvent, StateEvent, PreviewEvent ou
AskQuestion, rodar de backend/:

    uv run python tests/fixtures/contract/gen_api_samples.py
"""
from __future__ import annotations

import sys
from pathlib import Path
from typing import get_args

HERE = Path(__file__).resolve().parent
OUT = HERE / "api_samples"
# Rodado como script, o pacote `app` não está no caminho de import: a raiz é backend/.
sys.path.insert(0, str(HERE.parents[2]))

from app.models import (  # noqa: E402
    AskOption,
    AskQuestion,
    AskQuestionItem,
    ChatEvent,
    ChatKind,
    PreviewEvent,
    ShellVivo,
    StateEvent,
)


def _models() -> dict:
    cases = {
        "chat_minimal": ChatEvent(kind="user_msg", id="u1"),
        "chat_full": ChatEvent(
            kind="tool_use", id="toolu_01", text="acentuação e emoji 🚀", tool_name="Bash",
            tool_input={"command": "ls -la", "nested": {"list": [1, 2.5, None, True, "x"]}},
            tool_use_id="toolu_01", result="ok", is_error=False, ts=1727712000.123,
            cache_read=1234, cache_ttl_s=3600, desistiu=True, hook_error="hook recusou",
            skill={"name": "pdf", "path": "/skills/pdf/SKILL.md", "body": "# PDF"},
            orq={"kind": "woke", "task": 4, "body": "texto", "alarm": False},
            queued_delivered=True, queued_ts=1727712001.5, queued_confirmed=False, image_count=2,
            offset=999,  # exclude=True: não aparece no JSON
        ),
        "state_minimal": StateEvent(session="s1", state="idle"),
        "state_full": StateEvent(
            session="s1", state="awaiting_input", codex_mode="plan",
            codex_question={"provider": "codex", "request_id": 7, "is_async": False, "questions": []},
            codex_buffering=True, claude_permission_mode="acceptEdits",
            claude_previous_non_plan="default",
            claude_plan_pending={"plan": "# Plano", "path": "/tmp/p.md", "tool_use_id": "toolu_9"},
            label="Pensando…", question="Continuar?", options=["Sim", "Não"],
            status_line="opus · 12%", overlay=True, login=True, limited=True, limit_reset="3pm",
            loop_status="rodando", loop_iter=2, loop_max=5, problema="turno_com_erro",
            problema_detalhe="detalhe", headless=True, recarregar_motivo="config",
            shells=[ShellVivo(pid=42, cmd="sleep 999", desde=1727712000.5),
                    ShellVivo(pid=43, cmd="tail -f x")],
        ),
        "preview_minimal": PreviewEvent(session="s1", text=""),
        "preview_full": PreviewEvent(session="s1", text="**negrito** e acentuação", md=True,
                                     full=True, vivo=True),
        "ask_minimal": AskQuestion(questions=[]),
        "ask_full": AskQuestion(questions=[AskQuestionItem(
            header="Escolha", question="Qual caminho?", multiSelect=True,
            options=[AskOption(label="A", description="primeiro", preview="```\nA\n```"),
                     AskOption(label="B")],
        )]),
    }
    # Um por tipo: tipo novo no models.py vira amostra nova, e o teste Rust cobra a variante.
    for kind in get_args(ChatKind):
        cases[f"chat_kind_{kind}"] = ChatEvent(kind=kind, id=f"{kind}-1", text="x")
    return cases


def samples() -> dict[str, str]:
    """Nome do arquivo -> conteúdo exato, como o backend serializa (`model_dump_json`)."""
    return {f"{name}.json": model.model_dump_json() + "\n" for name, model in _models().items()}


def main() -> None:
    OUT.mkdir(exist_ok=True)
    want = samples()
    for old in OUT.glob("*.json"):
        if old.name not in want:
            old.unlink()
    for name, text in want.items():
        # newline="\n": gerado no Windows sai igual ao do Linux.
        (OUT / name).write_text(text, encoding="utf-8", newline="\n")
    print(f"{len(want)} amostras em {OUT}")


if __name__ == "__main__":
    main()
