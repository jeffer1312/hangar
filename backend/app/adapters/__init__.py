"""Registro de providers: sse.py/registry.py pedem o Adapter da sessao por aqui em vez de
instanciar TranscriptTailer/StateMonitor/terminal_input direto."""
from app.adapters.claude import ClaudeAdapter
from app.adapters.codex.adapter import CodexAdapter
from app.adapters.kimi.adapter import KimiAdapter
from app.adapters.pi.adapter import PiAdapter

PROVIDERS = {"claude": ClaudeAdapter(), "codex": CodexAdapter(), "pi": PiAdapter(),
             "kimi": KimiAdapter()}


def get_adapter(provider: str):
    return PROVIDERS[provider]
