"""Textos que o app injeta nas sessões de um grupo (protocolo, entrada, saída, tarefa).

Stdlib-only: o hook de SessionStart (hooks/pair_hook.py) importa daqui pra reinjetar o
protocolo depois de /clear, e um import de app.config puxaria pydantic pra dentro de um hook
que roda a cada abertura de sessão — mesma regra do engines.py."""


def _tarefa(task: str) -> str:
    return f" na tarefa: {task.strip()}" if task.strip() else ""


def _lista(nomes: list[str]) -> str:
    return ", ".join(f"'{n}'" for n in nomes)


def texto_grupo(me: str, others: list[str], task: str, contrato: str | None) -> str:
    """Protocolo completo. `contrato` = caminho do markdown compartilhado, ou None quando há par
    remoto (o contrato não sincroniza cross-server) ou o grupo não tem gid."""
    exemplo = others[0]
    linha_contrato = (
        f"Contrato/decisões que o grupo precisa consultar: registrar no arquivo compartilhado "
        f"{contrato} (markdown; criar se não existir, manter curto e atual). ") if contrato else ""
    return (
        f"[de: hangar] GRUPO DE TRABALHO ATIVO: você ('{me}') trabalha junto com {_lista(others)}{_tarefa(task)}. "
        f"Cada sessão mexe SÓ no próprio repo; quando precisar de algo de outro membro (contrato, "
        f"endpoint, tipo, dúvida), mande 1:1 por iniciativa própria. COMO mandar, nesta ordem: "
        f"se você TEM a ferramenta SendMessage e o membro aparece no seu ListAgents (sessão Claude "
        f"desta máquina), use SendMessage — a entrega é por socket, sem digitar no terminal, então "
        f"nada de texto cortado ou colado pela metade. Não tem a ferramenta, ou o membro não está "
        f"na lista (sessão de outra máquina 'servidor::sessao', Codex, Pi)? Aí é o Bash: "
        f'hangar-send {exemplo} "sua mensagem". Os dois chegam do mesmo jeito, como [de: <membro>]. '
        f'AVISO pro grupo TODO (marco: "terminei minha parte", "contrato atualizado"): '
        f'hangar-send --group "sua mensagem" — ele entrega pelo terminal a quem não tem caminho nativo '
        f"e, se sair com código 3, lista a quem você ainda precisa mandar por SendMessage, com o texto "
        f"exato [grupo: {me}] ... . "
        f"REGRA ANTI-LOOP: NUNCA responda um [grupo: ...] com --group (vira tempestade; o backend "
        f"recusa). Aviso de grupo é unidirecional; se precisar responder, faça 1:1 e só se necessário. "
        f"{linha_contrato}"
        f"BRANCH: antes de trabalhar, rode git branch --show-current no SEU repo e alinhe pra "
        f"branch do ticket da tarefa (fetch+checkout) — re-verifique após restart/resume da sessão. "
        f"Exceção única: o usuário pedir explicitamente outra branch. Checkout DUPLICADO do repo "
        f"na máquina → alerte o usuário e pergunte qual é o canônico antes de mexer. "
        f"Commit/push e decisões de rumo continuam com o usuário. Confirme em uma linha."
    )


def texto_entrada(novos: list[str], membros: list[str], task: str) -> str:
    """Uma linha pra quem JÁ estava no grupo: o protocolo ele já tem."""
    return (f"[de: hangar] {_lista(novos)} entrou no seu grupo de trabalho{_tarefa(task)}. "
            f"Membros agora: {_lista(membros)}. Mesmo protocolo de sempre (1:1 por SendMessage/hangar-send; "
            f"--group só pra marco). Não precisa responder.")


def texto_saida(quem: str, motivo: str, resto: list[str]) -> str:
    """Pra quem ficou. `resto` = os OUTROS que ainda estão com o destinatário."""
    fim = (f"O grupo continua entre você e {_lista(resto)}." if resto
           else "O grupo foi dissolvido (só restava você); volte a operar independente.")
    return f"[de: hangar] '{quem}' {motivo}. {fim}"


def texto_tarefa_atualizada(task: str) -> str:
    return f"[de: hangar] Tarefa do grupo atualizada para: {task.strip()}. Não precisa responder."
