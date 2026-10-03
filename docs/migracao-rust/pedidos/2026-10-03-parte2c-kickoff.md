# Parte 2C da migração para Rust — planejar e executar

Você vai PLANEJAR e, em seguida, EXECUTAR a etapa 2C: observação das sessões **com terminal** do
Claude e do Codex no Rust (tmux em modo controle `-C` e cópia da tela por sessão). O dono autorizou
executar logo depois de planejar, sem parar para aprovação.

## Onde trabalhar

- Pasta: `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte2c`, branch
  `hangar-server-parte2c`, criada a partir da `hangar-server-parte1` (`a80a9e45`), que é a branch do
  PR #24 e já contém a parte 1 e a 2A.
- Outras sessões trabalham em paralelo: `rust-parte2` planeja a 2B (sem terminal no Rust) na pasta
  `hangar-server-parte2`. Não mexa em outra pasta. Se a 2C precisar de algo que a 2B também vai
  mudar (contrato interno, `side-events`), registre o ponto de encontro nas notas do plano.

## Leia antes

1. Roteiro: `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte1/docs/superpowers/specs/2026-10-02-migracao-rust-roteiro.md`.
2. Análise e spec da parte 2 (seção 2C):
   `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte2/docs/analise-hangar-server-parte2-2026-10-02-claude-codex.md` e
   `/home/jefferson/pessoal/hangar/.claude/worktrees/hangar-server-parte2/docs/superpowers/specs/2026-10-02-hangar-server-parte2-design-claude-codex.md`.
3. `CLAUDE.md` da raiz e as "Regras vigentes" de `docs/decisoes/harnesses.md` e `docs/decisoes/windows.md`
   (psmux, códigos de retorno). Estado e prévia de terminal têm regras próprias lá.
4. Código: `backend/app/state.py`, `preview.py`, `tmux.py`, `terminal_input.py`, `hook_state.py`,
   `sse.py`, `internal_api.py`, `rust_server.py`; e `crates/hangar-server` (`side`, `tail`, `routes`).

## Escopo da 2C

- Ler a tela das sessões com terminal sem criar um processo `tmux` por leitura: uma conexão `tmux -C`
  contínua, com a tela de cada sessão mantida em memória (`alacritty_terminal`, já usado no
  `desktop-native`), e o mesmo resultado de estado/prévia que o Python produz hoje
  (`classify`, spinner, statusline, overlay, rate limit, prévia).
- Prova de paridade como na parte 1: fixtures de pane que já existem (`backend/tests/fixtures/pane_*.txt`)
  passando pelo Python e pelo Rust com o mesmo resultado.
- Reserva: sem `tmux -C` (ou se ele cair), o Python continua lendo como hoje. Windows/psmux: confirme
  o suporte a `-C` no código do psmux; se não houver como provar, o Windows fica no Python.
- Fora: envio de teclas (é a 2D), sessões sem terminal (2B), Pi/Kimi/omp/orq, lista, quadro, canvas.

## Como trabalhar

1. Escreva análise curta, spec e plano em `docs/superpowers/specs|plans/2026-10-03-hangar-server-parte2c*.md`
   (formato do plano da parte 1: `### Task N:` / `- [ ] **Step N: …**`, código completo, testes primeiro).
   Não commite spec nem plano (`docs/superpowers/` é ignorado de propósito).
2. Execute Task a Task: testes focados de cada Task autorizados (nunca a suíte inteira); commit por Task
   com `git add` de caminhos explícitos e mensagem descritiva em inglês; marque cada Step `[x]` no plano.
   Ao fim de cada Task, faça uma revisão independente do diff dela (como fez na 2A) antes de seguir.
3. Mudou o contrato interno Python↔Rust → suba `RUST_SERVER_PROTOCOL` e `INTERNAL_PROTOCOL` juntos.
4. Não faça push. Ao terminar (ou se travar numa decisão que só o dono toma), mande um resumo curto via
   `hangar-send hangar "…"`: commits, testes, o que não conferiu.

## Regras

- Nunca suba, reinicie ou pare o backend nem o serviço `hangar-backend` (há um vivo; subir outro mata
  as sessões sem terminal). Nunca rode instaladores. Medições avulsas em `/tmp`, apagadas no fim, sem
  conversa real no repositório. Um servidor tmux de teste em socket próprio (`tmux -L <nome>`) é
  permitido para medir e testar; nunca toque no tmux do usuário.
- Log nunca leva texto de conversa nem conteúdo de tela. Identificadores novos em inglês; comentários
  em português, curtos, sobre o porquê. Afirmação técnica com prova (arquivo:linha) ou medição.
