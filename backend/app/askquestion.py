import json
from typing import Optional
from app.models import AskQuestion


def parse_ask_question(jsonl: str) -> Optional[AskQuestion]:
    """Le o jsonl do transcript e devolve o payload do ULTIMO tool_use AskUserQuestion (perguntas
    estruturadas), ou None. Robusto a linhas malformadas. A decisao de SE oferecer (sessao realmente
    aguardando input) fica na camada de estado/SSE; aqui so extrai a estrutura."""
    found = None
    try:
        with open(jsonl, "r", encoding="utf-8", errors="replace") as fh:
            for line in fh:
                line = line.strip()
                if not line or "AskUserQuestion" not in line:
                    continue
                try:
                    obj = json.loads(line)
                except (json.JSONDecodeError, ValueError):
                    continue
                content = ((obj.get("message") or {}).get("content")) or []
                if not isinstance(content, list):
                    continue
                for block in content:
                    if (isinstance(block, dict) and block.get("type") == "tool_use"
                            and block.get("name") == "AskUserQuestion"):
                        inp = block.get("input") or {}
                        try:
                            found = AskQuestion.model_validate(inp)
                        except Exception:
                            pass
    except OSError:
        return None
    return found
