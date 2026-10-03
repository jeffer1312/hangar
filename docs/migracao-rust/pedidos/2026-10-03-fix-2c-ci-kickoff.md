# Corrigir os testes da 2C que falham no Windows e no macOS (CI)

A 2C (leitura da tela das sessões com terminal pelo Rust, `tmux -C`) passa no Linux mas falha no
`cargo test --locked --workspace` do workflow `.github/workflows/server.yml` nos jobs Windows e macOS
(run `37109525007` no GitHub `jeffer1312/hangar`; ver também `37109324277`). Exemplos que falham no
macOS: `another_session_pane_and_changed_active_target_fail_without_substitution`,
`actual_alternate_screen_capture_and_numeric_session_keep_exact_target`,
`canonical_capture_matches_tmux_history_ansi_join_blanks_and_literal_percent`,
`command_timeout_active_eof_protocol_error_and_bad_utf8_discard_observer`,
`read_only_control_observer_never_subscribes_to_pane_output` (e outros dos testes de terminal).
Como esses dois jobs têm `continue-on-error`, a release das duas plataformas não é atualizada e
lá o Python continua atendendo sozinho.

## Onde trabalhar

Pasta `/home/jefferson/pessoal/hangar/.claude/worktrees/fix-2c-ci`, branch `fix-2c-ci`, criada da
`hangar-server-parte1` (`cd15bcfa`, branch do PR #24).

## O que fazer

1. Ache a causa com evidência: `gh run view <id> --repo jeffer1312/hangar --log-failed` para os
   dois jobs. Hipóteses a confirmar ou descartar: o runner não tem `tmux` (macOS) ou não existe tmux
   (Windows) e os testes sobem um tmux real; diferença de comportamento do tmux do macOS (versão,
   `-C`, formatos); caminhos/sockets.
2. Corrija pela causa real, mantendo o desenho: a 2C é só para Linux/Unix com tmux (no Windows o
   Python atende — `cfg!(windows)`/`win32` já desliga). Teste que depende de um tmux real deve ser
   pulado com motivo claro quando o tmux não estiver disponível, ou o CI deve instalar o tmux onde
   fizer sentido (macOS: `brew install tmux`) — escolha o que prova mais sem falso verde. Testes
   puros (reducer, parser) devem rodar em todas as plataformas.
3. Prove localmente o que der: `cargo test --locked --workspace` em `crates/` (Linux) e
   `cargo check --locked --target x86_64-pc-windows-gnu -p hangar-server --tests` (o alvo Windows
   está instalado). Depois faça commit e **push da branch `fix-2c-ci`** para o GitHub rodar o
   workflow nas três plataformas; acompanhe com `gh run watch` e só termine com os jobs Windows e
   macOS verdes (ou com a causa provada de que não dá, se for o caso).
4. Não mexa em `main` nem na `hangar-server-parte1`; quem junta é a sessão `hangar`. Ao terminar,
   mande via `hangar-send hangar "…"`: causa, conserto, commits e o link da execução verde.

## Regras

- Nunca suba, reinicie ou pare o backend nem o serviço `hangar-backend`; nunca toque no tmux do
  usuário (servidor de teste só com `tmux -L <nome>`); nunca rode instaladores.
- Commits com mensagem descritiva em inglês, `git add` de caminhos explícitos. Identificadores
  novos em inglês; comentários em português, curtos, sobre o porquê.
