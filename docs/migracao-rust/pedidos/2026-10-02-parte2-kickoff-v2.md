# Parte 2 (escopo novo) — sessões do Claude e do Codex, com e sem terminal

O dono mudou o escopo da parte 2. Pi, Kimi, omp, orq, lista de sessões, quadro e canvas ficam
FORA: ele não usa Pi/Kimi/omp agora, e a lista mediu ganho pequeno. O que ele usa o tempo todo são
sessões do **Claude** e do **Codex**, com e sem terminal. A leitura das conversas delas (histórico e
chat ao vivo) já está no Rust desde a parte 1. Esta parte leva para o Rust o resto do caminho delas.

Mantém valendo tudo do pedido anterior (`2026-10-02-parte2-kickoff.md`): onde trabalhar, o que ler,
regras, formato da spec e do plano, parar para aprovação, não implementar. Seu trabalho anterior
(análise, spec e plano de Pi/Kimi/omp/orq e lista) fica como referência; escreva arquivos novos com
sufixo `-claude-codex`.

## Escopo

**Sem terminal (headless):**
- `backend/app/adapters/claude_headless/` (adapter, conexão com o cano, stream-json, prévia
  `pensamento`/`ferramenta`, permissões e perguntas, retomada após reinício do backend).
- `backend/app/adapters/codex/` no modo sem terminal (`sem_terminal.py`, app-server JSON-RPC pelo
  cano, aprovações por cartão, `codex_question`).
- Envio de mensagens e fila para essas sessões (`pqueue`, `drain`, confirmação), estado e prévia.
- Conserto conhecido: `_input_parcial` (`claude_headless/adapter.py`) reparseia o tool input inteiro a
  cada pedaço — 200 KB custam 0,73 s de laço travado (quadrático); `previa +=` também é quadrático.

**Com terminal (tmux):**
- Estado (`state.py`: `capture-pane` a cada 0,75 s e `classify`), prévia (`preview.py`, a 0,15 s
  enquanto trabalha), status line, envio por `terminal_input.py`/`tmux.py`, Codex com TUI `--remote`
  e app-server WebSocket por sessão (`adapters/codex/`).
- Avaliar o desenho que a análise apontou: tmux em modo controle (`tmux -C`, uma conexão longa em vez
  de um processo por leitura; 11× menos CPU medido) e cópia da tela por sessão
  (`alacritty_terminal`, já usado no `desktop-native`). Conferir se o psmux (Windows) aceita `-C`.

## O que decidir e mostrar na análise

- O que vale migrar agora e o que deixar no Python (as CLIs mudam muito: ~1/3 dos commits do backend
  são correção de CLI externa — `docs/decisoes/harnesses.md` tem ~56 regras, ~51 valem em qualquer
  linguagem). Proponha a divisão em subpartes se ficar grande demais para um plano só, com a ordem.
- Medições reais (sem subir backend) do custo de hoje por sessão com e sem terminal.
- Como a reserva no Python continua valendo (o Rust cai, o Python assume) e como o contrato interno
  muda (subir `RUST_SERVER_PROTOCOL`/`INTERNAL_PROTOCOL`).
- Os quick wins que podem ir no Python já (ex.: o quadrático do `_input_parcial`) se forem mais
  baratos que portar.

Entregue análise, spec e plano (`…-claude-codex`), uma mensagem curta ao dono em português com a
recomendação e UMA pergunta, e pare.
