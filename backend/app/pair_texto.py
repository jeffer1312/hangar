"""Textos que o app injeta nas sessões de um grupo (protocolo, entrada, saída, tarefa).

Stdlib-only: o hook de SessionStart (hooks/pair_hook.py) importa daqui pra reinjetar o
protocolo depois de /clear, e um import de app.config puxaria pydantic pra dentro de um hook
que roda a cada abertura de sessão — mesma regra do engines.py."""


# Aviso do app, não de uma sessão: `[de: X]` fazia o modelo responder pra sessão X (e o socket nativo
# põe X como remetente). O espaço garante que nunca coincide com um nome de sessão (names.py).
PREFIXO = "[painel: grupo de trabalho]"


def _tarefa(task: str) -> str:
    return f" na tarefa: {task.strip()}" if task.strip() else ""


_NOME_HARNESS = {"claude": "Claude Code", "codex": "Codex", "pi": "Pi", "omp": "omp", "kimi": "Kimi Code"}
# Só estes carregam o MCP `hangar`; Pi, omp e Kimi falam com o grupo pelo CLI.
_COM_MCP = {"claude", "codex"}


def _lista(nomes: list[str], harness: dict[str, str] | None = None) -> str:
    """'b' (Codex), 'c' — sem rótulo quando o harness é desconhecido (par remoto, hook)."""
    harness = harness or {}
    def um(n: str) -> str:
        rotulo = _NOME_HARNESS.get(harness.get(n, ""), "")
        return f"'{n}' ({rotulo})" if rotulo else f"'{n}'"
    return ", ".join(um(n) for n in nomes)


def _como_mandar(provider: str, exemplo: str) -> str:
    cli = f'no shell, hangar-send {exemplo} "msg" (1:1) e hangar-send --group "msg" (aviso pro grupo todo)'
    if provider in _COM_MCP:
        como = (f"COMO MANDAR: 1:1 pela tool `send` do MCP hangar (alvo = nome da sessão); aviso pro "
                f"grupo todo pela tool `group`. Sem essas tools nesta sessão: {cli}.")
    else:
        como = f"COMO MANDAR: {cli}."
    if provider == "claude":
        como += " Não use SendMessage: o ListAgents mostra apelidos e ele recusa o nome real da sessão."
    return f"{como} Aviso de grupo é só pra marco (\"terminei minha parte\", \"contrato atualizado\")."


def texto_grupo(me: str, others: list[str], task: str, contrato: str | None,
                harness: dict[str, str] | None = None) -> str:
    """Protocolo completo. `contrato` = caminho do markdown compartilhado, ou None quando há par
    remoto (o contrato não sincroniza cross-server) ou o grupo não tem gid. `harness` = nome ->
    provider; o de `me` escolhe o jeito de mandar, o dos outros só rotula a lista."""
    harness = harness or {}
    linhas = [
        f"{PREFIXO} GRUPO DE TRABALHO ATIVO: você, {_lista([me], harness)}, trabalha junto com "
        f"{_lista(others, harness)}{_tarefa(task)}.",
        "- Cada sessão mexe SÓ no próprio repo. Precisou de algo de outro membro (contrato, endpoint, "
        "tipo, dúvida)? Mande 1:1 por iniciativa própria.",
        f"- {_como_mandar(harness.get(me, 'claude'), others[0])}",
        "- Chega do outro lado como [de: <membro>] (1:1) ou [grupo: <membro>] (aviso), seja qual for "
        "o harness dele.",
        "- ANTI-LOOP: NUNCA responda um [grupo: ...] com outro aviso de grupo (o backend recusa). "
        "Responder, só 1:1 e se necessário.",
    ]
    if contrato:
        linhas.append(f"- CONTRATO: decisões que o grupo precisa consultar vão no arquivo compartilhado "
                      f"{contrato} (markdown; criar se não existir, manter curto e atual).")
    linhas += [
        "- BRANCH: antes de trabalhar, rode git branch --show-current no SEU repo e alinhe pra branch "
        "do ticket da tarefa (fetch+checkout); re-verifique após restart/resume. Exceção única: o "
        "usuário pedir outra branch. Checkout DUPLICADO do repo na máquina → pergunte ao usuário qual "
        "é o canônico antes de mexer.",
        "- Commit/push e decisões de rumo continuam com o usuário. Este aviso é do app, não de uma "
        "sessão: confirme em uma linha aqui mesmo, sem hangar-send.",
    ]
    return "\n".join(linhas)


def texto_entrada(novos: list[str], membros: list[str], task: str,
                  harness: dict[str, str] | None = None) -> str:
    """Uma linha pra quem JÁ estava no grupo: o protocolo ele já tem."""
    return (f"{PREFIXO} {_lista(novos, harness)} entrou no seu grupo de trabalho{_tarefa(task)}. "
            f"Membros agora: {_lista(membros, harness)}. Mesmo protocolo de sempre (1:1; aviso de grupo "
            f"só pra marco). Não precisa responder.")


def texto_saida(quem: str, motivo: str, resto: list[str]) -> str:
    """Pra quem ficou. `resto` = os OUTROS que ainda estão com o destinatário."""
    fim = (f"O grupo continua entre você e {_lista(resto)}." if resto
           else "O grupo foi dissolvido (só restava você); volte a operar independente.")
    return f"{PREFIXO} '{quem}' {motivo}. {fim}"


def texto_tarefa_atualizada(task: str) -> str:
    return f"{PREFIXO} Tarefa do grupo atualizada para: {task.strip()}. Não precisa responder."
