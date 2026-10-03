# Parte 2 da migração para Rust — pedido para a sessão Codex

Você vai ANALISAR e PLANEJAR a parte 2 da migração do backend do Hangar para Rust. Não implemente
nada ainda: o dono aprova o plano antes da execução.

## Onde trabalhar

- Pasta: `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte2`, branch
  `hangar-server-parte2`, criada a partir da `hangar-server-parte1` (commit `f7f797bb`).
- A `hangar-server-parte1` ainda recebe consertos de CI de outra sessão. Antes de planejar trechos
  que dependam dela, rode `git fetch origin` e confira o que mudou em
  `origin/hangar-server-parte1`.

## Leia antes (nesta ordem)

1. `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte1/docs/superpowers/specs/2026-10-02-migracao-rust-roteiro.md`
   — como a migração funciona, estado das 7 partes, lições da parte 1, pendências.
2. `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte1/docs/analise-backend-rust-2026-10-02.md`
   — análise e medições que motivaram a migração.
3. Spec e plano da parte 1 (modelo de formato e de profundidade):
   `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte1/docs/superpowers/specs/2026-10-02-hangar-server-conversas-design.md`
   e `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte1/docs/superpowers/plans/2026-10-02-hangar-server-conversas.md`.
4. O `CLAUDE.md` da raiz do repositório e as "Regras vigentes" de `docs/decisoes/harnesses.md`,
   `docs/decisoes/plataforma.md` (entrada "hangar-server") e `docs/decisoes/windows.md`.
5. O código já portado em `crates/hangar-server` (rotas, `side`, `tail`, `transcript`, `auth`,
   `proxy`) e o lado Python (`backend/app/internal_api.py`, `rust_server.py`, `sse.py`).

## Escopo da parte 2

- **Lista de sessões:** `GET /api/sessions` e o streaming da lista (`list_events` em `sse.py`,
  `_ListRefresher`, `registry.list()`/`list_with_state`, filtros de convidado `guest_safe`/
  `filter_visible`, `shortcut_terminals`, `nav`). Hoje é o maior gasto por tick com muitas sessões.
- **Conversas do Pi, Kimi, omp e orq:** `/history` e `/events` dessas sessões, que na parte 1
  continuaram repassados ao Python inteiros.
- **Fora:** o quadro (board) e o canvas da web desktop — o app nativo não tem e o desktop web está
  congelado. Não porte nada específico deles.
- Se a análise mostrar que algo do escopo deve ficar para outra parte (ou entrar nesta), diga e
  justifique com medição.

## O que entregar

1. **Análise** do escopo, medida no sistema real quando der (sem subir backend): o que cada rota
   faz, custo hoje, o que o Rust ganha, o que depende de tmux/`/proc`/sidecars, os invariantes que
   não podem mudar para os clientes (web, celular, nativo, peers) e os riscos.
2. **Spec** em `docs/superpowers/specs/2026-10-0X-hangar-server-parte2-design.md` (data do dia), no
   mesmo formato da spec da parte 1: o que muda, fora do escopo, linha de base, como funciona,
   como provar, pronto quando.
3. **Plano** em `docs/superpowers/plans/2026-10-0X-hangar-server-parte2.md`, no formato do plano da
   parte 1: `### Task N:` e `- [ ] **Step N: …**` (casado por regex — não é livre), Files,
   Interfaces, código completo de cada passo, testes primeiro, passos "Rodar … (quando autorizado)",
   commits com `git add` de caminhos explícitos.
4. Uma mensagem final curta para o dono, em português, com o resumo, o que você recomenda e as
   decisões que dependem dele (uma pergunta por vez, opções letradas).

## Regras

- **Não implemente nada** nesta etapa. Não commite spec nem plano (a pasta `docs/superpowers/` é
  ignorada pelo git de propósito).
- Nunca suba, reinicie ou pare o backend nem o serviço `hangar-backend` — há um vivo, e subir outro
  mata as sessões sem terminal. Nunca rode instaladores.
- Testes automatizados só quando o dono pedir (regra do projeto); medições avulsas com scripts
  temporários em `/tmp` são permitidas, apagadas no fim, sem copiar conversa real para o repositório.
- Mantenha as decisões da parte 1: estrangulamento com reserva no Python, contrato interno versionado
  (`RUST_SERVER_PROTOCOL`/`INTERNAL_PROTOCOL`), Linux em glibc 2.28 via `cargo zigbuild`, log sem
  texto de conversa, só o token do dono é atendido pelo Rust (convidado e peer sempre repassados).
- Identificadores novos em inglês; comentários e textos ao usuário em português.
- Afirmação técnica vem com prova do código atual (arquivo:linha) ou medição.
