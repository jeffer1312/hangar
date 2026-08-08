# As cinco incógnitas do envio no Windows, medidas

Medido em 08/08/2026 na máquina `winboat` (psmux 3.3.7, commit `05cc5d4` de 20/07/2026), pela sessão
`cockpit-2`, contra uma sessão Claude Code viva num pane psmux e a sessão `medicao-clip` criada pelo
próprio app. Nenhuma medição apertou Enter — nada foi submetido a nenhuma sessão.

Este arquivo é a Task 0 do plano `docs/superpowers/plans/2026-08-08-envio-windows.md`. Ele existe
porque três destas cinco medições **decidem** o código, e duas delas podiam cancelar o plano inteiro.
Resultado: as cinco passam, e o plano fica de pé — com quatro correções de rumo, listadas no fim.

## (a) O processo da tarefa agendada escreve o clipboard que a TUI lê? **Sim**

Esta era a que matava o plano. O backend no Windows nasce de uma tarefa agendada, não de um
PowerShell interativo — se ele não alcançasse o clipboard do desktop, as Tasks 1–3 seriam inúteis.

A sessão `medicao-clip`, criada pelo app (logo, filha do backend da tarefa agendada), rodou
`Set-Clipboard -Value 'DA-TAREFA-777'`; o `Get-Clipboard -Raw` de um PowerShell interativo devolveu
`DA-TAREFA-777` em 8 s. O caminho existe.

## (b) `send-keys M-v` é literalmente o comando que cola? **Sim — mas só com a sessão ociosa**

`M-v` cola: rc=0 e o texto do clipboard aparece no composer.

**As três primeiras tentativas falharam, e não foi a tecla.** A sessão estava processando
(`Churned for 27s`) e **a TUI descarta a tecla enquanto trabalha**. Repetida com ela ociosa, colou na
hora. Sem um guard de "posso digitar agora?" antes do `M-v`, a colagem cai no vazio em silêncio —
que é exatamente a falha que este plano existe para acabar.

Duas variantes que devolvem **rc=0 e não colam** (mais `rc=0` mentiroso do psmux, a mesma família do
bug do `-` documentado em `tmux.py:479-490`):

| Comando | Cola? |
|---|---|
| `send-keys M-v` | sim |
| `send-keys -l` com o literal ESC + `v` | sim |
| `send-keys Escape v` (uma chamada só) | **não**, rc=0 |
| `send-keys C-v` | **não** |

## (c) `clip.exe` serve no lugar do `Set-Clipboard`? **Serve, mas perde para ele**

Os quatro encodings testados (UTF-16LE e UTF-8, com e sem BOM) dão rc=0 e preservam acento, cedilha e
emoji — o `clip.exe` fareja o encoding sozinho, até sem BOM.

Duas coisas medidas que mudam o código:

- **Não mandar BOM.** Quando vai, ele vira **conteúdo literal**: o primeiro caractere lido é U+FEFF,
  invisível no terminal e dentro do que a sessão recebe. O bloco A do plano (`b"\xff\xfe" + ...`)
  está errado por isso.
- **`clip.exe` converte LF em CRLF, sempre.** Medido byte a byte com `linha1\nlinha2\nlinha3`:
  `clip.exe` devolve 22 bytes com `\r\n`; `Set-Clipboard` devolve 20 bytes com `\n`, idêntico ao
  original. Nenhum dos dois acrescenta newline no fim.

Decisão: **`Set-Clipboard`/API .NET é o caminho principal**, por ser exato. `clip.exe` fica como
plano B, e nesse caso o código assume CRLF.

## (d) Quanto demora o `[Pasted text #N]` numa mensagem de 600 linhas? **665–1341 ms, média 922**

Clipboard carregado com 600 linhas via `Set-Clipboard` (13.799 chars), composer limpo com `C-u`,
cronômetro a partir do `send-keys M-v`, poll contínuo de `capture-pane` (sem sleep; um `capture-pane`
custa ~26 ms, e esse é o piso da resolução) até casar `Pasted text #`.

Cinco repetições na mesma sessão ociosa: **676, 665, 955, 1341, 975 ms** — mínimo 665, máximo 1341,
média 922. As cinco trouxeram `+599 lines` e conteúdo íntegro.

(A primeira medição, de amostra única, deu 597 ms com poll de 50 ms. Era a mais rápida e ficou de
fora: uma amostra não mostra cauda.)

Duas consequências, e as duas são de código:

- **Nada de sleep fixo.** O orçamento de hoje (`_MULTILINE_SUBMIT_SETTLE = 0.5` +
  `_SUBMIT_CHECK_PRAZO = 1.0`, `terminal_input.py:36,51`) veio do `paste-buffer` do Linux: o sleep de
  0,5 s falharia em **100% das amostras** — até a mais rápida levou 665 ms — e o teto de 1,5 s já foi
  raspado a 1341 ms em só cinco tentativas. Mantido, toda mensagem cairia em `partial`, gastaria os 2
  requeues e morreria com o clipboard funcionando. O código espera o placeholder por **poll**.
- **O tempo cresce com o número de colagens na mesma sessão.** Repetições 1 e 2 na casa dos 670 ms;
  3 a 5 entre 955 e 1341, com o contador indo de `#2` a `#6`. Numa sessão de uso longo, 3 s pode
  ficar apertado — então o poll leva teto generoso e, **ao estourar, falha alto**: nunca manda Enter
  às cegas. Um Enter sem placeholder submete a mensagem pela metade, que é o bug que este trabalho
  existe para matar.

O placeholder veio como `[Pasted text #1 +599 lines]` — 1 linha exibida + 599, **as 600 inteiras, sem
perda**. O caminho de hoje, no mesmo teste, entrega 309 de 600.

O rodapé muda para `paste again to expand`: **um segundo `M-v` expande, não recola**. O código nunca
pode mandar `M-v` duas vezes achando que reforça.

## (e) O `C-u` limpa a colagem colapsada? **Sim**

Com o composer ainda carregado da colagem de (d), um único `C-u` limpou: nenhum `Pasted text #` no
pane, composer de volta à linha vazia. Reconfirmado depois das cinco colagens da segunda rodada. `_limpar_composer` (`terminal_input.py:879`) funciona lá — e
isso importa porque, com o caminho novo, `partial` deixa de ser exceção rara e vira caminho quente.

## O que muda no plano

1. **Task 1** usa `Set-Clipboard`, **sem BOM**. O bloco A (`clip.exe` + BOM UTF-16LE) sai; `clip.exe`
   vira plano B assumindo CRLF.
2. **Task 2** espera a sessão aceitar entrada **antes** do `M-v` — o guard de duas capturas do
   `state.classify`, o mesmo que o `model_picker` usa (uma captura só não separa spinner vivo de
   turno terminado).
3. **Task 2** prova a entrega esperando o placeholder por **poll** (teto generoso, ≥3 s), manda `M-v`
   **uma vez só**, e ao estourar o teto **falha alto** — sem Enter às cegas.
4. **Fora do plano, achado no caminho:** o próprio `cp-send`/`input` mutila o comando rumo ao Windows
   — a primeira medição de (a) morreu com `unexpected EOF while looking for matching quote` (aspa
   aberta sem fecho, barra solta), e a string de teste de (c) chegou **sem acento e sem emoji**, em
   ASCII puro. Não está no plano e precisa de entrada própria.
