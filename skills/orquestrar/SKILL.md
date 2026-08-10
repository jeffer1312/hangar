---
name: orquestrar
description: |
  Orquestra uma tarefa que atravessa VARIOS repos usando sessoes Claude pareadas do claude-pocket - esta sessao vira a LIDER, cria/pareia uma sessao visivel por repo (cp-send --new/--pair), escreve o contrato compartilhado do grupo, distribui o escopo, monitora os reportes e consolida o painel final pro usuario. Use quando o usuario pedir "orquestra", "coordena", "distribui essa tarefa/ticket entre os repos", "abre uma sessao pra cada repo", ou quando um ticket multi-repo precisar de trabalho paralelo em mais de um repo. NAO use para: tarefa de um repo so (sessao normal), exploracao read-only multi-repo (subagent Explore resolve), ou mandar um recado avulso (cp-send direto).
---

# orquestrar — sessão líder de um grupo de trabalho multi-repo

Esta sessão vira a **líder**: decompõe a tarefa, cria uma sessão pareada **visível**
por repo (nunca subagent pra editar outro repo — o usuário quer ver os terminais
trabalhando), escreve o contrato do grupo, distribui, monitora e consolida.

Toda a mecânica vem do claude-pocket e **já está pronta** — a skill só rege:

| Mecânica | Quem faz |
|---|---|
| Criar sessão visível (tmux + --session-id) | `cp-send --new <nome> <cwd>` |
| Grupo de N sessões + protocolo injetado | `cp-send --pair <sessao> "<tarefa>"` (grupos se fundem) |
| Regras de conduta (só-meu-repo, anti-loop, branch, push=usuário) | prompt que o backend injeta no pair — **não reexplicar** |
| Contrato compartilhado (PairSheet na UI) | `~/.claude/.claude-pocket-pair/grupo-<gid>.md` |
| Recado 1:1 / aviso de marco | `cp-send <sessao> "..."` / `cp-send --group "..."` |

Subagent continua ok pra **leitura** (explorar outro repo, rastrear fluxo). Edição
fora do cwd desta sessão = sessão pareada, sempre.

## Procedimento

### 1. Escopo (antes de criar qualquer sessão)

Levantar e **confirmar com o usuário**:
- Tarefa: chave do ticket (`ABC-1234`) ou descrição livre. Ticket → branch = chave, e as regras de ticket do CLAUDE.md valem em todas as sessões.
- Repos afetados: cwd de cada um + o que cada sessão vai fazer (1 linha por repo).
- Interfaces entre repos que precisam ser combinadas (rota, payload, evento, tipo).

Escopo incerto → perguntar, não adivinhar. Criar sessão é ação visível na máquina
do usuário; só depois do ok.

### 2. Sessões — reusar antes de criar

```bash
cp-send --list        # nome, estado, cwd das sessões vivas
```

- Já existe sessão viva no cwd do repo → **reusar** (perguntar ao usuário se ela está livre pra tarefa).
- Não existe → `cp-send --new <tarefa>-<repo> <cwd>` (ex: `abc-1234-api-web`). Nunca `tmux new-session` cru.

### 3. Parear (uma chamada por sessão)

```bash
cp-send --pair <sessao> "<tarefa>"
```

Grupos se fundem sozinhos — parear N sessões uma a uma resulta num grupo único.
O backend injeta o protocolo em cada membro (inclusive nesta); não repetir as regras
no kick-off.

### 4. Contrato

Path: ler `gid` do próprio sidecar e montar o caminho —

```bash
gid=$(python3 -c "import json;print(json.load(open('$HOME/.claude/.claude-pocket-pair/<minha-sessao>.json'))['gid'])")
# contrato: ~/.claude/.claude-pocket-pair/grupo-$gid.md
```

Escrever (curto, vivo — o PairSheet exibe ele):

```markdown
# Grupo <tarefa> — <resumo>

## Escopo por sessão
- **<sessao-a>** (<repo>, branch <X>): <o que faz> — status: pendente
- **<sessao-b>** ...

## Interfaces combinadas
- <rota/payload/evento acordado entre repos>

## Decisões
- <decisão que o grupo precisa consultar>
```

Status e decisões se atualizam no MESMO arquivo conforme o trabalho anda; membro
também escreve na própria seção.

### 5. Kick-off (1:1, um por sessão)

```bash
cp-send <sessao> "Tua parte na <tarefa>: <escopo específico>. Contrato: <path>. Ao terminar: roda a suíte completa, reporta a contagem (ex: 341/341) via cp-send <minha-sessao>. Bloqueio ou dúvida de interface → me manda 1:1."
```

Só o escopo específico — protocolo geral já foi injetado no pair.

### 6. Monitorar

- Reportes chegam como `[de: <sessao>]` → atualizar o status no contrato e resumir pro usuário nos marcos.
- Sessão calada não é sessão travada: `cp-send --list` mostra o estado (`working`/`idle`/`awaiting_input`) — só cutucar se `idle` sem reporte ou `awaiting_input` (avisar o usuário: pode estar esperando permissão no terminal dela).
- Marco global ("contrato atualizado", "back fechou") → `cp-send --group "..."` UMA vez.
- Decisão de rumo/escopo aparecendo no meio → usuário decide, não a líder nem o par.

### 7. Consolidar

Todas reportaram → painel final pro usuário:

| Sessão | Repo | Branch | Testes | Pendência |
|---|---|---|---|---|

Push/MR/Jira **sempre** ficam com o usuário (regras do CLAUDE.md valem em cada
sessão — a líder não manda ninguém pushar). Usuário deu ok → repassar a ordem 1:1
por sessão e cobrar o hash+branch+status de volta.

Fim da tarefa: avisar o grupo (`--group`, uma vez), perguntar ao usuário se desfaz
o pareamento (`cp-send --unpair` em cada sessão é decisão dele). O contrato fica —
é o registro do que foi combinado.

## Limites

- Um repo só → não usar a skill; sessão normal resolve.
- Sessão remota (`servidor::sessao`) recebe recado 1:1 mas **não entra em grupo** (pareamento cross-server não existe) — coordenar na mão via 1:1 e registrar no contrato local.
- Líder também trabalha? Pode — no PRÓPRIO repo, seguindo o mesmo contrato.
