# Parte 4: achados pendentes

Achados médios e baixos das revisões por Task que entraram na `feat/parte4` sem conserto (ritmo
"juntar primeiro, corrigir depois"). Cada item sai daqui quando for corrigido, com o hash.

## Antes da `hangar-server-parte1`

- Task 10: prova na VM DELPHI-02 (Step 23) com web e nativo. Os testes `term::conpty::*` e os
  `cfg(windows)` da Task 7 passaram no job Windows do CI (run 37467170394). O nativo responde ao
  `ESC[6n` (`desktop-native/src/term_view.rs:56-58`, `Event::PtyWrite`); o xterm.js também.
  Prova automática na VM (06/10, `7a210b6a`, contrato 32, psmux, cliente WS por script):
  abre em 70 ms, eco e resize ok, fechar não mata a sessão nem digita nada nela. Dois achados:
  (a) sem resposta ao pedido de cursor (`ESC[6n`, efeito do `INHERIT_CURSOR` do `portable-pty`)
  o terminal não aceita tecla nenhuma — xterm.js e nativo respondem, mas qualquer cliente que não
  responda fica mudo; (b) tecla enviada logo depois do primeiro byte (antes de ~3 s) se perde
  durante a partida do `tmux attach`; falta comparar com o Python. Falta a prova manual com web
  e nativo numa sessão Claude da VM. Decisão da coordenação: (a) só registrado (os dois clientes
  respondem); (b) corrigido em `f858952dd`: no Windows a entrada fica segurada até o psmux
  pintar (`ESC[?1049h`, ~80 ms), com prazo de 5 s e teto de 64 KiB; o Python tinha o mesmo
  defeito (janela de ~10 ms) e o Linux não perde. Conferido na VM (06/10, `641838a3`, binário
  compilado na VM, contrato 35): 20 aberturas digitando no instante da abertura, o comando chegou
  inteiro ao pane em 19; em 1 (primeiro byte em 281 ms, partida lenta) sumiu uma letra no meio
  (`PRVA` em vez de `PROVA`). Resíduo raro só para tecla mandada nos primeiros ~100 ms; causa não
  provada (psmux/conhost trocando o modo de entrada na partida é a suspeita).
  Prova manual do dono (06/10, app nativo pela VM por RDP): terminal abre, digita e mostra o
  Claude; achou lento. Medido: eco de tecla ~20 ms no painel e ~19 ms no `tmux attach` direto
  (é o psmux), saída grande igual com e sem o Hangar (`medicao.md`); o dono atribuiu ao RDP.
  O botão de terminal externo não existe no Windows (só emuladores do Linux; não é da parte 4).
- Task 5: no Windows a prévia pelo pane captura a 0,15 s com um processo psmux por toque
  (~25–50 ms cada) enquanto a sessão trabalha sem arquivo do hook; medir na VM e, se pesar,
  limitar o ritmo rápido no Windows. O Monitor no Windows (psmux) e o convidado de convite de
  verdade não foram conferidos (só o Connect).

## Limpeza depois da junção

- A ponte `terminal_observer` → `POST /__hangar_server/terminal` ficou sem consumidor em todo
  modo (Task 11): código morto no Python (`terminal_observer.lease/capture`) e no Rust
  (`terminal_routes`, rota privada). Sai na limpeza ou na parte 7.
- `plugin_bridge.modo_sem_dialogo` usava o quadro do `StateMonitor` (`state.shared_capture`); sem
  ele, cada poll de permissão segurada captura o pane de novo (um processo por poll, só enquanto há
  permissão segurada). Ler o modo do `Monitor` do Rust ou do retrato dos fatos.

- VM DELPHI-02 roda um `hangar-server` compilado nela (`crates/target/release`, de `641838a3`)
  porque o binário Windows da `feat/parte4` não foi publicado (CI do Windows caindo em testes de
  tempo do #82 e dos custos). Quando a release tiver o Windows da versão juntada: apagar o
  `crates/target` da VM, Atualizar, e voltar o `CP_UPDATE_BRANCH` dela para `hangar-server-parte1`.

## Fora da parte 4, achados pela prova (Task 12)

- ~~`/select` no Codex sem terminal responde 500~~: corrigido em `427eb0d5d` (a rota do terminal
  só vale para `claude`; resultado incerto vira 409 `erro_sem_confirmacao_resposta`), provado com
  a prova real (Step 27, Codex sem terminal).
- Codex 0.159.3 mudou o rodapé do `/permissions` (`enter select · esc back`);
  `codex_permissions.py` não reconhece e responde 409 `erro_permissao_picker`.
- Codex com terminal com menu de aprovação na TUI fica `working` na lista e no chat: o Python só
  lê `menu_codex` antes de existir thread (estado do Codex é da parte 5).

## Médios e baixos

- Task 1: envio de fatos que falha não é reenviado sozinho (o Rust recupera pelo retrato a cada
  25 s ou pelo pulo de sequência); lote com o Rust travado atrasa até 2 s por sessão; `_down`
  global; `_seq`/`_sent` sem poda; fim de `transfer_active` não avisa (o Rust usa `session.dead`,
  conferido na hora); corpo da rota `state-service` lido antes do teto; empurrão acima de 64 KiB
  (pergunta com resumo enorme) é recusado pela ponte.
- Task 2: cache do `permission.observe` pode ficar velho se o Python trocar o modo por outro
  caminho até o pane mudar; `None` do pane do agente fica 60 s em cache (igual ao Python).
- Task 3: a captura rápida traz a análise completa do pool, refeita no caminho com corte; clone
  do texto do hook por toque; `norm()` sem os separadores `\x1c-\x1f`; a época zera a prévia sem
  publicar vazio (o reset do hub cobre); largura por `unicode-width` pode divergir do Python em
  caracteres raros.
- Task 4: com `awaiting_input` e sem `ask_question` emitido, o `Monitor` relê o arquivo da
  pergunta a cada rodada (o Python só relê na mudança de estado) — desvio aceito: o hook pode
  gravar depois de o menu aparecer; problema do runtime perde para o dos fatos e da observação
  (igual ao Python); o celular não mostra o detalhe dos códigos novos; casamento degradado vai
  ao log em `info` (igual ao Python); nativo não compilado nesta Task (só o `messages`).
- Task 5: pane do agente cai em `=nome:` quando a descoberta falha (igual ao Python, só log);
  `RuntimeView` não limpa quando o ator fecha sem evento; prévia do hook ilegível cai no pane só
  com log; linha acima de 1 MiB no canal privado derruba o stream do convidado (reconecta);
  `Drop` sem runtime não solta o consumidor do pool.
- Task 6: depois do Esc o hook é cancelado sem `/ask-fim` e a pergunta segurada vale até 35 s:
  chat e lista mostram o cartão e o clique cai na tecla do pane (aberto em `harnesses.md`;
  conserto no `perm.ts` ou no `/interrupt`); mapa do estado publicado sem idade (`Monitor`
  travado deixa a lista com o último estado); `held` da última resposta boa com o Python fora
  (linha marcada `list_facts_unavailable`); session-id lido na hora de publicar, não o da rodada;
  exceção em `_held` derrubaria todos os fatos; com `Monitor` vivo a lista não pede rebaixamento
  do registro nativo. O Python de reserva (`registry.list_with_state`) tem o mesmo defeito do
  cartão e não foi mexido.
- Task 8: fila de entrada limitada em quadros (64), não em bytes; `lock().unwrap()`; erro de
  leitura do PTY que não é EIO fecha como fim normal; resize do PTY que falha só aparece em debug;
  `restore_after_crash` na subida sem diário (só `warn`); troca de painel com desmontagem acima de
  10 s fica com dois painéis (vai ao diário).
- Task 10: `Drop` implícito do `Pty` (future cancelada, pânico) solta o mestre antes de matar o
  filho no Windows; no ConPTY a saída não dá EOF quando o `tmux attach` sai
  (microsoft/terminal#4564), e `exit` deixa o painel mudo até o cliente fechar (igual ao Python);
  o caminho `forget` vaza a vaga do teto de painéis; `fail()` no Windows usa `child.wait()` sem
  prazo se o `TerminateProcess` falhar; erro do `try_wait` vira `client_not_reaped` sem log;
  `dropping_writer_writes_nothing` não separa "nada escrito" de "conhost morreu no EOF".
- Task 9: rota privada do terminal responde 400/404 sem linha de log própria no Rust (o Python
  registra o status); quadro do cliente sem teto próprio no Python (Rust e uvicorn limitam); em
  `pending` o `/api/config` publica a capacidade do Python; braço morto `TermActive` no
  `execute`.
- Task 7: psmux pode escrever erro no stdout com código diferente de 0 e virar "quadro" (conferir
  na VM, Task 10/13); a linha da lista mostra só `list_capture_failed` e o código fino só vai ao
  log (já era assim); pior caso de 5 s + 5 s quando captura e `has-session` estouram; stderr e
  `io::Error` descartados (só o código); statusline e limite congelam sem `problema` na falha (já
  era assim).
