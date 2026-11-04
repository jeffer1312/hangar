"""Registro de providers: sse.py/registry.py pedem o Adapter da sessao por aqui em vez de
instanciar TranscriptTailer/StateMonitor/terminal_input direto."""
from app.adapters.claude import ClaudeAdapter
from app.adapters.claude_headless import sessions as headless_sessions
from app.adapters.claude_headless.adapter import CHAVE as CLAUDE_HEADLESS, ClaudeHeadlessAdapter
from app.adapters.codex.adapter import CodexAdapter
from app.adapters.kimi.adapter import KimiAdapter
from app.adapters.omp.adapter import OmpAdapter
from app.adapters.pi.adapter import PiAdapter

# "claude-headless" é chave INTERNA: a sessão continua `provider="claude"` (é Claude para o
# front, comandos, estatísticas e cotas); só o transporte muda. Quem precisa do adapter certo
# passa pelo `chave_de(...)`, que olha o sidecar sem terminal.
PROVIDERS = {"claude": ClaudeAdapter(), CLAUDE_HEADLESS: ClaudeHeadlessAdapter(),
             "codex": CodexAdapter(), "pi": PiAdapter(), "kimi": KimiAdapter(), "omp": OmpAdapter()}


def get_adapter(provider: str):
    return PROVIDERS[provider]


def chave_de(name: str, provider: str) -> str:
    """Chave do adapter de uma sessão: o provider dela, salvo Claude sem terminal."""
    if provider == "claude" and headless_sessions.exists(name):
        return CLAUDE_HEADLESS
    return provider
