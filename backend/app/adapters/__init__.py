"""Registro de providers: sse.py/registry.py pedem o Adapter da sessao por aqui em vez de
instanciar TranscriptTailer/StateMonitor/terminal_input direto."""
from app.adapters.claude import ClaudeAdapter

PROVIDERS = {"claude": ClaudeAdapter()}


def get_adapter(provider: str):
    return PROVIDERS[provider]
