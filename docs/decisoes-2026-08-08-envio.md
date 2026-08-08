# Decisões tomadas sem o usuário — plano `2026-08-07-envio-por-pty`

Ele estava indisponível durante a execução. Pediu que as decisões fossem tomadas pelo caminho
recomendado e anotadas aqui para revisar depois. Quatro decisões — três já previstas no plano, uma
quarta veio de um achado da revisão na Task 3.

## 1. O caminho por PTY descartável foi cortado do plano

A versão anterior do plano criava `app/ptysend.py` (~200 linhas) para escrever bracketed paste num
`tmux attach` descartável. Medição de 08/08: `tmux load-buffer -` resolve o único ganho que sobrava
(o teto de 16 KB) em uma linha — **1,088 MB em 0,32 s**, mais rápido que os 459 ms que o PTY levava
para 1 MB — sem `fork`, sem cliente anexado, sem dimensionar janela. E o PTY tem um problema que o
`load-buffer` não tem: `attach` entrega no pane **ativo** do cliente tmux, enquanto o envio mira
`_pane_target`, o pane do **agente** especificamente (`tmux.py:103`, função `_pane_target`). Numa sessão com split manual, o
PTY iria mandar o texto pro shell do dono em vez do agente.

**Alternativa se discordar:** construir o `app/ptysend.py` mesmo assim. O plano anterior está no
git (`git show HEAD~1 -- docs/superpowers/plans/2026-08-07-envio-por-pty.md`, depois do commit desta
Task 4) e as medições do PTY continuam registradas em `docs/polish-backlog.md`.

## 2. A limpeza confirmada agora autoriza um requeue no `drain`, com teto de 2 tentativas

Sem isso, a Task 1 (limpar o composer antes de reportar `partial`) trocaria "mensagem duplicada" por
"mensagem apagada e nunca enviada": o `drain` da fila durável hoje para no `partial` justamente
porque nada limpava a linha do composer (`terminal_input.py:560-567`). Com a limpeza confirmada, faz
sentido reenfileirar e tentar de novo — mas sem teto, uma falha persistente (ex: TUI que nunca
aceita bracketed paste) giraria para sempre. Teto de 2.

**Alternativa se discordar:** não mexer no `drain`, limpar só no caminho interativo (`/input`
direto). A fila durável continuaria com o comportamento de hoje — concatenação quando o reenvio
esbarra em resíduo não limpo.

## 3. Mensagem curta continua sem limpeza — decisão de não mexer

`_composer_residuo` exige `_RESIDUO_MIN = 12` caracteres sem espaço (`terminal_input.py:260`), então
uma mensagem tipo "ok, pode fazer" nunca é reconhecida como resíduo nosso e o `C-u` de limpeza não
dispara. É deliberado: abaixo desse tamanho não dá pra distinguir com segurança o que é resíduo
nosso do que o dono pode ter digitado por cima. Consequência honesta: para mensagem curta, o
`partial` continua se comportando exatamente como antes deste plano — nenhuma piora, mas também
nenhum conserto.

**Alternativa se discordar:** baixar `_RESIDUO_MIN`, ao custo de eventualmente apagar texto que o
dono tinha digitado no composer por engano de reconhecimento.

## 4. `_run` do tmux usa o alias `RUN`, não `subprocess.run` direto — texto do plano corrigido na hora

Na Task 3 (trocar `set-buffer`+`send-keys -l` por `load-buffer -`), o texto do próprio plano
prescrevia `patch("app.tmux.subprocess.run")` para o teste do ramo novo. A revisão pegou que isso
furava `RUN = subprocess.run` (`backend/app/tmux.py:7`) — o alias que é a **única** costura de mock
do módulo, usado em ~50 lugares em 5 arquivos de teste. Seguir o texto do plano à risca deixaria
`load-buffer` como o único comando de tmux fora dessa costura: um teste futuro que parecesse
verde poderia estar batendo no tmux real sem ninguém notar.

Decidido pelo caminho recomendado: o ramo novo usa `RUN`, o teste virou `patch.object(tmux, "RUN")`,
e o `test_tmux_paste.py` voltou ao ponto de patch original com só a asserção do verbo trocada
(commit `425fb1b`).

**Alternativa se discordar:** reverter para `patch("app.tmux.subprocess.run")` como o plano original
pedia — funciona, mas reabre o furo na costura de mock que a revisão fechou.

## Achados menores diferidos

Registrados durante a execução, não consertados de propósito — são menores e entram na revisão
final da branch, não neste plano. Do `.superpowers/sdd/2026-08-07-envio-por-pty/progress.md`:

- **Task 1** — `terminal_input.py:860` (`_limpar_composer`, checagem inicial): a checagem inicial de
  tri-estado não tem teste; mutar `is not True` para `is False` passa a suíte inteira.
- **Task 1** — `terminal_input.py:862-865` (`_limpar_composer`, laço): dentro do laço, o `C-u`
  continua saindo quando a releitura devolve `None`; sair no primeiro `None` seria mais fiel a
  "nunca limpar às cegas".
- **Task 1** — `_ULTIMA_LIMPEZA` é sidecar global e o comentário promete uma exclusividade que o
  `_send_lock` (por nome, solto antes da leitura) não garante — `api.py:1316` (função `_send_one`)
  já documenta `/input` e `drain` concorrentes na mesma sessão. Vira bug real se algo passar a ler
  esse sidecar fora do lock.
- **Task 2** — `terminal_input.py:569-574`: se a entrada some entre o claim e o requeue,
  `bump_attempts` devolve 0 e o ramo do requeue roda mesmo assim — o log chega a afirmar
  "reenfileirado (tentativa 0/2)". Conserto sugerido: `if limpou and 1 <= tentativas <=
  _PARTIAL_MAX_TENTATIVAS`.
- **Task 2** — `terminal_input.py:569-571`: um `OSError` de disco escapa do `drain`, diferente do
  ramo `deferred` 25 linhas abaixo, que já tem `try`/`except OSError` com comentário explicando por
  quê.
- **Task 2** — `terminal_input.py:566-568`: o `threading.local` só é seguro enquanto `_partial` for
  o único produtor de `"partial"`; qualquer chamador fora do `drain` que escreva nele nunca apaga.
  Correção de uma linha: apagar o atributo antes do `send_prompt` (linha 554), para que ausência
  volte a significar "não passou pelo `_partial`".

Task 3 não tem item diferido: o único achado menor que apareceu ali (`isinstance` moldado por mock
mal tipado) foi consertado no mesmo round de revisão que fechou o achado da decisão 4 acima
(commits `7e37414..425fb1b`), não ficou pendente.
