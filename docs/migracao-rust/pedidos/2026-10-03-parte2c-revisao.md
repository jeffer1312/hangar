# 2C — achados das revisões independentes (corrigir antes de entrar no PR #24)

Duas revisões do diff `a80a9e45..d21a445b`: uma geral e uma de falhas silenciosas. Paridade dos
reducers, troca de protocolo e segurança (cliente `read-only,ignore-size`, listener privado) estão
certas. Corrija TUDO abaixo na mesma branch `hangar-server-parte2c`, com teste de regressão primeiro
(falhando antes, passando depois), testes focados, commits por item, sem push. Ao terminar, avise a
sessão `hangar` com hashes, testes e a medição do item 6.

## Regra que decide o item 6

A 2C existe para ler a tela gastando menos que o Python. **Ela só entra no PR se, medida por rodada
e por chat aberto, custar menos CPU e menos processos que o caminho Python de hoje** (um
`capture-pane` por rodada). Meça antes e depois (CPU do backend + Rust + tmux por rodada, número de
comandos tmux e de chamadas HTTP), com um servidor tmux de teste (`tmux -L`). Se não ficar mais
barata, simplifique até ficar — por exemplo: sem a grade `alacritty_terminal` enquanto ela não muda
nenhum resultado, uma captura por rodada pela conexão `-C`, e o reducer rodando onde a captura já
está (sem ida e volta HTTP para refazer um cálculo puro). Registre a medição em `docs/decisoes/harnesses.md`.

## Importantes

1. **Prévia morre após `/clear` com dois aparelhos no Python** — `backend/app/terminal_observer.py:215`
   (`retired`), `preview.py:613`, `state.py:862`. Conexões percebem o `/clear` em tempos diferentes,
   `retired()` dá True e o laço da prévia retorna; `subscribe()` só recria a tarefa com `_task is None`
   (`preview.py:711`). Divergência de sessão deve pular a rodada, nunca encerrar o laço (ou recriar a
   tarefa morta no `subscribe`). O revisor reproduziu só com Python.
2. **`tmux -C` aparece como cliente anexado** — `crates/hangar-server/src/terminal_control.rs:340`;
   `scripts/hangar-panel-data:62` (`attached_locally`) passa a marcar toda sessão com chat aberto e
   `scripts/hangar-panel-open:100` pode focar a janela de OUTRA sessão (o painel do Hyprland roda
   nesta máquina, `hangar-panel.service`). Filtre `#{client_control_mode}` nos dois scripts e em
   qualquer outro ponto que liste clientes (confira o fallback `display-message -p '#S'` do
   `hangar-send`).
3. **Falha persistente abre um `tmux -C` novo por rodada, sem pausa** — qualquer erro mata o actor
   e a rodada seguinte (0,75 s por chat) anexa de novo, disparando `client-attached/detached`.
   Causas: tmux sem `display-message -l` (`:389`; confira a versão mínima), pane > 65 536 células
   (`:145`), resize na captura. Pausa crescente por sessão nos dois lados, ou checar a versão uma vez.
4. **Rust travado deixa o estado lento em vez de cair no Python** — `TIMEOUT=6.0` por `capture` e por
   `reduce` (`terminal_observer.py:20`) no executor padrão do Python (o mesmo do sidecar da prévia e
   do git): ~13 s por rodada e executor esgotado com poucos chats. No Rust, `enqueue`
   (`terminal_control.rs:239-251`) espera até 5 s segurando o mutex global. Desligue a ponte por um
   tempo após N falhas (como `MAX_CRASHES`) e não espere dentro do lock.
5. **Lease do Codex sem uso** — `backend/app/adapters/codex/adapter.py:1504-1507` anexa `tmux -C`
   a toda sessão Codex de terminal com chat aberto sem nunca capturar; a grade estoura `MAX_FRAME`
   (`:154`) e o actor renasce a cada 20 s. Tire o lease do Codex.
6. **Grade sem efeito e custo maior** — `terminal_control.rs:427`: os dois ramos analisam o mesmo
   texto; por rodada e chat são 12 comandos tmux + 2 HTTP contra 1 `capture-pane` antes. Ver a regra
   acima.

## Falhas silenciosas

7. **Queda do observador sem causa** (alta) — `terminal_routes.rs:92` (erro → 503) e `:74` (→ 400)
   descartam a causa (`terminal_control.rs:342` `map_err(|_| …)`); no Python `terminal_observer.py:92`
   junta 400/503/timeout/conexão recusada em `http_unavailable` e `_failure` (`:56`) avisa uma vez
   por código até o Rust reiniciar. Rust: logar o código do erro (sem pane nem conteúdo). Python:
   guardar o status HTTP no código, `diag.registrar` na entrada e na saída da reserva, zerar o aviso
   no primeiro sucesso.
8. **Heartbeat do lease engole exceção** — `preview.py:596-602` e `codex/adapter.py:1531`
   (`gather(..., return_exceptions=True)`); `Lease.watch` (`terminal_observer.py:190`) pode lançar
   via `tmux._pane_target`. Logar o tipo da exceção; `try/except` com log dentro de `watch()`.
9. **Lease derruba prévia e estado** — `async with lease(...)` (`preview.py:592`) e `source.start()`
   do `StateMonitor.stream` lançam quando `_pane_target` falha, antes do primeiro poll. Em
   `Lease.start()`, falha vira reserva: logar, marcar fechado, seguir sem Rust.

## Menores (registre; corrija se for barato)

- Limites do reducer fixos no Rust (`terminal_state.rs:387` `>= 3`, `:392` `< 4`): passe pelo
  contrato ou teste que falhe se o Python mudar `STALE_LIMIT`/`IDLE_DEBOUNCE`.
- `_capture_and_store` chama `forget_frame` para outras sessões com quadro > 60 s e avança a época
  delas.
- Protocolo: a 2B também vai mudar o contrato; combine com ela (sessão `rust-parte2`) o número final
  quando as duas entrarem no PR — nunca dois contratos diferentes com o mesmo número.
