"""O contrato que o backend e o LANCADOR precisam concordar, palavra por palavra.

Existe porque estes tres valores sao lidos de dois processos diferentes: o backend (que abre a
thread pelo app-server) e `scripts/hangar-codex-tui` (que sobe a TUI no pane). Divergir aqui nao
daria erro nenhum — daria uma TUI com permissao diferente da que o resto do sistema supoe, calada.

Modulo separado, e nao o `adapter.py`, por causa do lancador: ele roda no pane a cada abertura de
sessao e importar o adapter arrastaria asyncio, websockets e o parser do transcript so pra ler tres
constantes.
"""

# clientInfo do handshake initialize (ver docs/codex-app-server-contract.md).
CLIENT_INFO = {"name": "hangar", "title": None, "version": "0.1.0"}
# Codex pode EDITAR arquivos no cwd da sessao -> workspace-write (nao read-only do spike).
SANDBOX = "workspace-write"
APPROVAL = "never"
