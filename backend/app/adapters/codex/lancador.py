"""O contrato do pane de uma sessao Codex: quem sobe a TUI, com que permissao, e como se apresenta.

Existe separado do `adapter.py` porque TRES processos diferentes precisam concordar nisto e nenhum
deles pode pagar o preco de importar o adapter (asyncio, websockets, parser do transcript):

- o backend, que cria a sessao (`registry.create` -> `CodexAdapter.spawn_command`);
- `scripts/hangar-codex-tui`, o lancador, que roda no pane a cada abertura de sessao;
- `scripts/hangar-codex`, o wrapper do `codex` do shell, que precisa criar a sessao mesmo com o
  backend desligado.

Divergir aqui nao daria erro nenhum — daria uma TUI com permissao diferente da que o resto do
sistema supoe, calada. Modulo so de stdlib, de proposito: o wrapper roda no `python3` do sistema.
"""

# clientInfo do handshake initialize (ver docs/codex-app-server-contract.md).
CLIENT_INFO = {"name": "hangar", "title": None, "version": "0.1.0"}
# Codex pode EDITAR arquivos no cwd da sessao -> workspace-write (nao read-only do spike).
SANDBOX = "workspace-write"
APPROVAL = "never"
# Nome do lancador no PATH (symlink do install-claude-wrapper.sh).
EXECUTAVEL = "hangar-codex-tui"


def comando_do_lancador(cwd: str, initial_prompt: str | None = None,
                        thread_id: str | None = None) -> list[str]:
    """O comando do pane de uma sessao Codex: o lancador unico, o MESMO nos tres chamadores.

    O nome da sessao nao entra aqui — `tmux new-session` carimba CP_SESSION_NAME no pane e o
    lancador le de la. Assim o comando nao repete a identidade que o tmux ja garante.

    `thread_id` retoma uma conversa que ja existe (o "Retomar" do Arquivo) em vez de abrir uma nova.
    """
    argv = [EXECUTAVEL, "--cwd", cwd]
    if thread_id:
        argv += ["--resume", thread_id]
    if initial_prompt:
        argv += ["--prompt", initial_prompt]
    return argv
