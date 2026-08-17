# Docs de domínio

Como as skills de engenharia devem consumir a documentação de domínio deste repo.

## Antes de explorar, leia isto

Este repo é de **contexto único**, mas o arquivo de domínio **não** é o `CONTEXT.md` do padrão das
skills — ele não existe aqui e não deve ser criado. O vocabulário, as decisões de desenho e as
armadilhas medidas vivem em:

- **`CLAUDE.md`** na raiz — 52 KB, é a bíblia do repo. A seção
  "Conventions & gotchas" carrega as decisões já tomadas, quase sempre com a medição que as
  justificou e a data.
- **`README.md`** — arquitetura e tabela de rotas da API.
- **`docs/USAGE.md`** — o produto do ponto de vista de quem usa.
- **`docs/polish-backlog.md`** — dívida conhecida e o motivo de ela ainda não ter sido paga.
- **`docs/superpowers/specs/`** — research por assunto, com `arquivo:linha`.

Não há `docs/adr/`. Uma decisão nova de arquitetura entra como parágrafo em `CLAUDE.md`, no mesmo
formato das que já estão lá: o que foi decidido, o que foi **medido**, e a data. Criar um segundo
lugar de verdade é exatamente o defeito que este repo está tentando consertar.

## Use o vocabulário que já existe

Quatro palavras deste domínio se confundem entre si. Use-as como definidas aqui:

| Termo | O que é | Onde mora |
|---|---|---|
| **servidor** | um backend Hangar a que **este celular** conversa | `localStorage`, chaves `cp_servers` / `cp_active` |
| **peer** | outra máquina que **este backend** alcança, para recado `servidor::sessao` | `backend/peers.json`, 0600, no disco do servidor |
| **conta** (ou perfil) | um config dir do Claude Code — `~/.claude-<nome>`, com credencial e histórico próprios | disco, marcador `.hangar-conta` |
| **motor** (engine) | um provedor não-Anthropic para uma sessão; só troca variáveis de ambiente | `~/.claude/engines.json` |

Sessão, pane e transcript também têm sentido fixo: **sessão** é a linha na lista do app,
**pane** é o retângulo do tmux, **transcript** é o `.jsonl` de onde vem o chat.

## Sinalize conflito com o que já está escrito

Se a sua proposta contradiz um parágrafo de `CLAUDE.md`, diga isso na cara em vez de sobrescrever
calado:

> _Contradiz o parágrafo "Config e opção moram em MODAL" do `CLAUDE.md` — mas vale reabrir porque…_

E o contrário também vale: **`CLAUDE.md` envelhece**. Verificado em 16/08/2026, a seção que diz que
a tela agregadora de Configurações "ainda não foi implementada" está errada — ela existe
(`settings/SettingsModal.svelte`), e três arquivos citados lá não existem mais. Achado negativo lido
no `CLAUDE.md` ("não existe", "ainda não") precisa ser conferido no código antes de virar plano.
