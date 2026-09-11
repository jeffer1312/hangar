"""Perguntas assíncronas e respostas no mesmo formato do histórico nativo do Codex."""
from collections import Counter

from .questions import response


class AsyncQuestions:
    def __init__(self, thread_id: str):
        self.thread_id = thread_id
        self._pending: dict[str, dict] = {}
        self._seen: set[str] = set()
        self._during_load: list[dict] | None = []
        self._resolved: set[str] = set()
        self._local_answers: dict[str, str] = {}
        self._echoes: Counter[str] = Counter()

    @property
    def count(self) -> int:
        return len(self._pending)

    def pending(self) -> dict | None:
        return next(iter(self._pending.values()), None)

    def observe(self, item: dict) -> bool:
        kind, item_id = item.get("type"), item.get("id")
        if not isinstance(item_id, str) or item_id in self._seen:
            return False
        if kind != "userMessage" and not (kind == "agentMessage" and item.get("delivery") == "async"):
            return False
        if kind == "agentMessage" and not item.get("questions"):
            return False
        self._seen.add(item_id)
        if self._during_load is not None:
            self._during_load.append(item)
        before = tuple(self._pending)
        if kind == "agentMessage":
            for index, question in enumerate(item.get("questions") or []):
                title, options = question.get("title"), question.get("options")
                if not isinstance(title, str) or not title.strip():
                    continue
                request_id = f"async:{self.thread_id}:{item_id}:{index}"
                if request_id in self._resolved:
                    continue
                self._pending[request_id] = {
                    "provider": "codex", "request_id": request_id, "is_async": True,
                    "questions": [{"id": "answer", "header": str(index + 1), "question": title,
                                   "multiSelect": False, "isOther": True, "isSecret": False,
                                   "options": [{"label": option, "description": ""} for option in options or []
                                               if isinstance(option, str)]}],
                }
        else:
            text = "".join(block.get("text", "") for block in item.get("content") or []
                           if block.get("type") == "text")
            if self._echoes[text]:
                self._echoes[text] -= 1
                return False
            # A TUI envia uma resposta por vez, citando o título e sem ID de resolução.
            for request_id, payload in self._pending.items():
                prefix = "> " + payload["questions"][0]["question"] + "\n\n"
                if text.startswith(prefix) and text[len(prefix):].strip():
                    self.resolve(request_id)
                    break
        return before != tuple(self._pending)

    def hydrate(self, thread: dict) -> None:
        restored = AsyncQuestions(self.thread_id)
        restored._during_load = None
        for turn in thread.get("turns") or []:
            for item in turn.get("items") or []:
                restored.observe(item)
        for item in self._during_load or []:
            restored.observe(item)
        for request_id, text in self._local_answers.items():
            restored.record_answer(request_id, text)
        self._pending, self._seen, self._resolved = restored._pending, restored._seen, restored._resolved
        self._local_answers, self._echoes = restored._local_answers, restored._echoes
        self._during_load = None

    def resolve(self, request_id: str) -> None:
        self._pending.pop(request_id, None)
        self._resolved.add(request_id)

    def record_answer(self, request_id: str, text: str) -> None:
        self._local_answers[request_id] = text
        if request_id in self._pending:
            self.resolve(request_id)
            self._echoes[text] += 1

    def response(self, request_id: str, answers: list[dict]) -> str:
        question = self._pending.get(request_id)
        if question is None:
            raise ValueError("A pergunta já foi respondida ou pertence a outra conversa.")
        value = response(question, answers)["answers"]["answer"]["answers"][0]
        return "> " + question["questions"][0]["question"] + "\n\n" + value
