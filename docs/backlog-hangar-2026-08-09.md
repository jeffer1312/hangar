# Pendências e achados — sessão de 2026-08-09 (rename para Hangar + marca na interface)

Escrito pra virar plano depois, não pra ser executado como está. Duas partes, de propósito:
**achados** são coisas medidas nesta sessão, com evidência e caminho de arquivo — não precisam de
investigação, precisam de decisão; **ideias** são o que ainda não foi analisado o bastante.

Contexto do que JÁ foi feito (pra não replanejar): rename do projeto e do repo pro Hangar, histórico
reescrito sem nomes internos, jogo de marca em `assets/brand/`, marca ligada na interface
(`components/icons/Hangar{Mark,Intro,Working}.svelte`), animação de trabalho substituindo o Lottie,
rail da esquerda refeito, painel de contexto ganhando recolher. Commits `88d4fce`, `55a723f`,
`8e10972`.

---

## Achados — medidos, com evidência

### 1. O trilho recolhido não pode ter controle clicável
`Sidebar.svelte:422` — `const expanded = $derived(!collapsed || hovering || ...)` e o
`onmouseenter` na linha 806: **passar o mouse já expande a barra**. Qualquer ícone no trilho é
inalcançável com mouse, porque o painel cresce antes do ponteiro chegar. Ou o trilho é só
informação, ou o hover deixa de expandir. É preferência, não certo/errado — daí a ideia da opção
em Aparência (ver ideias).

### 2. "Servidor ativo" é um modo invisível e consequente
A lista de sessões é **agregada de todos os servidores** (`lib/sessionsStore.svelte.ts`), então
trocar o ativo não muda nada do que se vê. O que ele decide, e só isso:
- onde nasce uma sessão nova — mas `CreateSessionSheet` já tem seletor próprio de alvo;
- **qual servidor o modal de Configurações edita** (as abas Notificações/Anexos/Avançado/Motores
  recebem um `alvo`);
- o destino de Reconectar e Sair.

Ou seja: estado que o usuário não vê, não consegue conferir, e que muda o efeito de outra tela.

### 3. O menu de conta é a única porta pra coisas que não são "conta"
`AccountMenu.svelte` reúne Servidores, Configurações, Horas silenciosas, Reconectar e Sair. Não há
outro caminho pra nenhum deles. O botão que o abre foi trocado por engrenagem nesta sessão, mas a
mistura (identidade de máquina + configuração + ações) continua.

### 4. O app não mostra versão em lugar nenhum
`SettingsModal.svelte` tem as seções App (Aparência, Ditado) e Servidor (Notificações, Anexos,
Avançado, Motores). Não existe "Sobre", nem versão, nem link do repositório.

### 5. `splash.json` é código morto e `lottie-web` continua declarado
O boot do chat virou esqueleto shimmer (`Chat.svelte:1571`) e a animação de montagem ficou órfã.
Depois da troca do indicador de trabalho, **nenhum componente importa Lottie** — o `lottie-web`
não aparece em nenhum chunk do build, mas segue em `frontend/package.json:29`, junto de
`lib/lottie/{pensando,splash}.json`.

### 6. `initials()` — duas limitações que ficaram
Consertada a colisão de irmãs (`svc-mailer-2` vs `svc-report-ai-2`), mas sobra:
- sufixo de **3+ dígitos** (`app-100`) cai no ramo genérico de duas letras;
- número **colado** sem separador (`app2` e `app3`) colide em "AP".

### 7. O venv da VPS está quebrado desde o rename ANTERIOR
Medido pela sessão da VPS: os shebangs dos console scripts apontam pra
`/home/jefferson/admin/hangar/...` — caminho que não existe desde o `pocket → cockpit`.
`backend/.venv/bin/pytest --version` falha. Ninguém percebeu porque a unit roda `uv run`, que
resolve o interpretador sozinho. Um `uv sync` conserta; está no roteiro da janela de rename de lá.

### 8. O auto-deploy da VPS quebrou por acoplamento de duas decisões certas
O commit do rename trocou os nomes de unit **no código** (`backend/app/deploy.py:27`,
`scripts/deploy.sh:21-22`) enquanto o roteiro proibiu renomear as units **na máquina**. Resultado:
webhook não acha `hangar-deploy.service` e o `deploy.sh` morreria no restart. Dois pushes engolidos
com 500 no journal. Some quando a janela de rename rodar lá.

### 9. Não foi conferido: identidade no celular
As mudanças de vocabulário visual (chip neutro + anel, superfície que segue a transparência,
engrenagem no lugar do avatar) foram feitas e verificadas **só no desktop**. `SessionCard.svelte` e
o cabeçalho de `SessionList.svelte` têm elementos próprios que podem ter ficado destoando.

### 10. Recado de pareamento fica parado no composer do Pi, e o reenvio duplica
Relatado e medido em 2026-08-09. Sintoma: recado que chega numa sessão **Pi** entra no composer e
**não é submetido**; destrava com um Enter manual no terminal, e a mensagem aparece duplicada.

Causa raiz, com evidência. `terminal_input.py:309` —
`_PASTE_ID_RE = re.compile(r"\[Pasted text #(\d+)")`. Esse é o placeholder do **Claude Code**. O Pi
colapsa paste também, com outro texto: **`[paste #1 1171 chars]`** (lido do pane do
`hangar-gpt-review`, pi 0.83.0, gpt-5.6-luna). Conferido no interpretador: a regex casa
`[Pasted text #3 +42 lines]` e devolve `[]` para a forma do Pi.

A cadeia inteira segue daí, e cada elo já está escrito no arquivo:

1. `_composer_residuo` (linha 330) procura a **cauda** ou o **começo** do texto na região do
   composer. Num paste colapsado o texto real nunca é desenhado — só o chip —, então nenhum dos
   dois casa.
2. O ramo que existe justamente pra isso (linha ~385, `if pastes_antes is not None and
   (_paste_ids(composer) - pastes_antes)`) depende da regex acima. No Pi ele nunca dispara.
3. Sem evidência, a função devolve `False`, `_entrou_no_composer` lê "não chegou" e **o Enter não é
   enviado**.
4. O remetente repete, e cada tentativa empilha OUTRO paste no composer — a duplicação que o
   usuário vê. Quando ele dá o Enter manual, a pilha drena de uma vez.

É o mesmo defeito já corrigido para o Claude Code em 31/07 (o comentário na linha 380 descreve a
mensagem quíntupla daquele episódio); a correção nunca foi estendida ao Pi, que na época ainda não
colapsava paste — ou não tinha sido medido.

**Por que funciona na maior parte das vezes.** O colapso só acontece acima de um certo tamanho.
Mensagem curta é desenhada literalmente no composer, a comparação por cauda/começo casa, e o Enter
vai — por isso o placar de hoje é de 135 `sent` em `hangar` contra 6 `deferred` em
`hangar-gpt-review`. Falha só a mensagem longa. Os dois casos capturados hoje têm **1032** e
**1171** caracteres; o limiar exato do Pi ainda não foi medido.

**A prova, no diagnóstico do próprio código** (journal, 21:44:56, `hangar-gpt-review-fresh`):

```
envio PARCIAL: multi-linha NAO chegou no composer em 1.0s — Enter nao enviado
composer='────…\n [paste #1 1032 chars]\n────…'  reguas=46,48  fundo=3  altura=2
pastes(antes=[] depois=[])
```

O composer **mostra** `[paste #1 1032 chars]` e o mesmo log diz `pastes(antes=[] depois=[])` — ou
seja, o texto chegou e o detector não viu nada. A região foi lida corretamente (as duas réguas
foram achadas, `reguas=46,48`); o que falhou foi só o reconhecimento do chip.

**O `deferred` é consequência, não causa.** Depois do primeiro envio travado o composer fica
ocupado, e o `_composer_ocupado_pi` (linha 1038) passa a adiar tudo que vem atrás — daí a fila que
drena de uma vez quando o usuário dá o Enter manual.

**CORRIGIDO em 09/08/2026** (ainda sem commit). `_PASTE_ID_RE` passou a aceitar os dois desenhos:
`\[(?:Pasted text|paste) #(\d+)`. A trava de identidade continua valendo — só um número NOVO em
relação à foto pré-paste conta, então rascunho do dono não vira prova da nossa entrega. Três testes
de regressão em `test_terminal_input.py` (chip do Pi novo conta; chip do Pi alheio não conta;
`_paste_ids` lê os dois desenhos). Suíte completa: 1660 passando.

Fica em aberto, e é medição, não código: **o limiar de colapso do Pi** (falhas vistas em 1032 e
1171 caracteres) e se a forma muda com `--no-extensions` — o composer do Pi já apareceu desenhado
de três jeitos diferentes neste arquivo (ver `_READY_MARKERS_BY_PROVIDER`, linha 193). Se aparecer
um quarto desenho, o lugar de acrescentar é o mesmo.

---

## Ideias — precisam de plano

### A. Abas no topo quando a sidebar recolhe (referência: OpenCode)
Em vez de trilho de iniciais, as sessões viram abas horizontais no topo, com **nome inteiro** em vez
de duas letras. Resolve o achado 1 de uma vez (sem trilho, não há hover). O que precisa ser
resolvido no plano: agrupamento por servidor/projeto se perde; overflow com 10+ sessões; relação com
board e canvas; e se é modo (barra recolhida → abas) ou substituição.

### B. Opção "expandir ao passar o mouse" (Aparência)
Padrão ligado, como é hoje. Desligado, o trilho vira barra de atividade clicável e os ícones fazem
sentido. É a saída barata pro achado 1 enquanto A não existe.

### C. Servidores para dentro de Configurações
Fim do menu intermediário: a engrenagem abre o modal direto, e a lista de servidores vira uma entrada
da seção Servidor, mostrando qual está sendo editado. Reconectar e Sair vão junto, que é onde fazem
sentido. Resolve os achados 2 e 3. Decidir no plano: trocar de servidor é ação frequente — ou fica
um clique mais cara dentro do modal, ou ganha um atalho próprio no rodapé.

### D. "Sobre" na configuração
Marca, nome, versão e link do repositório. Resolve o achado 4 e é o encaixe natural da marca numa
tela de config (decorar as abas com o símbolo seria enfeite).

### E. Separar grupos no trilho recolhido
No expandido existe agrupamento por servidor/projeto; no trilho, não. Marcar a troca de grupo com
espaçamento maior, sem texto. Só faz sentido se o trilho sobreviver ao plano A.

### F. Limpeza do Lottie
Remover `lottie-web` do `package.json`, os dois `.json` e o `components/Lottie.svelte`. Fecha o
achado 5. Barato, mas depois de confirmar as animações novas no uso real.

### G. Refinos da marca e da animação
- anel do plano no painel: desenhar o arco proporcional (24/31 = 77%) em vez de só escrever o número;
- avatar/header do X e o **social preview do GitHub** (upload manual, não tem API);
- registrar `hangar.dev` — é a única pendência com relógio correndo.

---

## Operacional em aberto

- **Renomear a pasta local** deste notebook (`Projetos/claude-cockpit` → `hangar`): exige fechar as
  sessões abertas no checkout, porque o script move o diretório debaixo delas. O
  `migrate-to-hangar.sh` já é a versão corrigida (leva transcripts, repara as 9 worktrees).
- **Janela da VPS**: update manual + rename de units/diretório + `uv sync`. Em execução.
- **winboat**: migrada e verificada; o diretório segue `C:\cockpit` de propósito (três pontos presos
  ao caminho: tarefas agendadas, os `.vbs` com `EncodedCommand` e o `hangar-send`).
- **macbook-jefferson**: quando ligar, `git pull` e depois `./scripts/migrate-to-hangar.sh`.
- **Tags `pre-hangar`** nas três máquinas: só saem depois da confirmação do app no celular com a
  build nova.
