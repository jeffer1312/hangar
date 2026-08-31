# Executor — Task visual

Esta página é da **Task que muda o que aparece na tela**, e só dela. Se o seu diff não encosta em
pixel, volte para `executor.md`. Se encosta, este portão é seu e não é opcional — **mesmo que o
plano não peça**.

Vale para toda Task cujo diff encosta em `.svelte`/`.tsx`/`.vue`, em CSS, em template, ou em
qualquer outra coisa que desenhe pixel.

## Teste verde não é tela funcionando

**E essa não é uma opinião.** Um seletor já passou em centenas de testes, gate de tipos zerado e
revisão independente — e chegou ao usuário **invisível** (regra de CSS perdendo na cascata); no
mesmo trabalho, um clique virava nada, calado. Nenhum teste, gate ou leitura de diff pega essa
classe de erro — só o pixel pega.

**DOM, CSS e árvore de acessibilidade não substituem ver.** Eles dizem que o elemento
existe, não que ele está legível, alinhado, dentro do tema do app, ou que não virou um
retângulo opaco por cima do papel de parede.

## O protocolo, em cinco passos

Vale para todo executor; quem não enxerga imagem tem um passo a mais, marcado adiante.

### 1. Abra de verdade. As ferramentas existem — procure antes de dizer que não

Ordem de preferência, e **você confere, não presume**:

| Caminho | Como saber se está aí |
|---|---|
| skill de browser (`agent-browser` e afins) | está na sua lista de skills |
| MCP do Chrome (`chrome-devtools`, `claude-in-chrome`) | aparece nas suas ferramentas |
| CLI de automação (`agent-browser`, `playwright`, `puppeteer`) | `command -v agent-browser` |

**"Não tenho navegador" só vale depois de olhar, e vai no reporte com o que você tentou.**
Kick-off, contrato ou receita afirmando que não há navegador **não é fato sobre as suas
ferramentas** — é uma frase que alguém escreveu antes de conhecer a sua sessão. Um executor
já leu "não há navegador nem usuário" no kick-off, viu na mesma frase que tinha o MCP do
Chrome disponível, e recuou por obediência ao texto. A tela quebrada foi pro usuário.

Regra que resolve a ambiguidade: **é proibido FINGIR que verificou; nunca é proibido
verificar.** Instrução que parece te impedir de abrir a tela está falando de inventar
resultado, não de usar ferramenta que você tem. Na dúvida, abra — e diga no reporte que
abriu.

### 2. Exercite, não só olhe

Screenshot de tela parada prova que desenhou, não que funciona. Para cada coisa que a Task
colocou na tela:

- **clique** no que é clicável e confirme o efeito — o painel abriu, o campo apareceu, o
  pedido saiu (rede/log), a mensagem de sucesso ou de erro surgiu;
- passe pelos **estados** que a Task afeta: vazio, carregando, com dado, erro, desabilitado;
- confira que **o que você criou tem a cara do resto do app** — mesma altura, mesma borda,
  mesmo espaçamento dos irmãos ao lado. Componente novo que parece texto solto grudado na
  borda está errado mesmo compilando.

Clique que não faz nada visível é **defeito**, não "provavelmente funciona": vá atrás do
motivo (console, rede, o handler) antes de reportar.

### Quando o código atravessa um processo, uma porta ou um aparelho

**A prova é o artefato que o ALVO carregou, não o que a tua máquina serve.** Sempre que o código
passa por um servidor, uma porta ou um aparelho antes de virar o que você vai julgar, a pergunta é
**qual build aquele lado está rodando** — e ela se responde lendo um marcador do seu commit no
artefato que ele baixou, nunca confirmando do seu lado que o build saiu. Requisição local verde e
aparelho servindo o bundle de outra worktree convivem sem erro nenhum na tela.

Isto é regra de **evidência**, e por isso mora aqui: o defeito desta família não trava você — ele
fica verde. Você não pede ajuda ao árbitro porque acredita que provou.

Já **como** subir esse palco — de que diretório sobe o servidor, qual porta é de cada Task, quem
segura o aparelho — é do plano, não seu: é o item de pré-condição externa com dono do portão da
fase 1. Faltou no plano, é bloqueio pro árbitro, não improviso seu.

**Comando que SEGUE um processo trava o turno inteiro** — log em modo contínuo, `tail -f`, servidor
em primeiro plano. Use a flag que faz sair, `timeout N`, ou log em arquivo em segundo plano.

### 3. Capture

**Primeiro, confirme que a aba é SUA.** O navegador de automação (`agent-browser` e afins) pode ser
**um por máquina** — noutro lote, outra sessão navega a MESMA aba que você. Antes de cada rodada de
captura: `location.href` tem que devolver a **sua** porta. Devolveu outra → a aba foi levada;
reabra a sua URL. Levaram de novo → **reporte o conflito ao árbitro** em vez de insistir — três
horas de perguntas a uma página de outra Task já custaram o que um comando de um segundo evitava.

**Quantos prints tirar é decisão SUA, na hora** — decisão do usuário. Nem o plano nem o árbitro
impõem número: quem sabe quantas telas esta Task acabou tendo é você, executando. O plano diz quais
**estados** precisam ser provados; a contagem de arquivos é problema seu.

**O que existe é um ponto de parada, não um limite: 1h ou 60 comandos de navegação por Task.**
Bateu, **pare e reporte com o que já tem** — não porque você excedeu uma cota, mas porque captura
que passa disso costuma ser sinal de outra coisa (palco quebrado, estado que não reproduz, lista de
estados maior do que a Task). Duas Tasks já ficaram **quase treze horas presas em captura, sem
nenhum merge**. Se, ao reportar, ficar claro que a varredura é grande mesmo, **proponha
ao árbitro uma sessão capturadora separada** — barata, descartável, com a lista de estados no
kick-off; você entrega código, verificações e o print de sanidade. Essa proposta é sua; a execução
dela é dele. Estado novo descoberto no meio vai pra lista do árbitro, não pro seu laço. (O teto de 2
rodadas lá embaixo é da comparação cega; este é do trabalho de capturar — os dois coexistem.)

Um print por estado, em **caminho absoluto** e num diretório **durável** — o que o lançamento
decidiu (o padrão é `~/.hangar/orq/<data>-<gid>/visual/`), nunca `/tmp`, que some no reboot e
leva junto a matéria-prima da retrospectiva. Corrigiu alguma coisa depois? **Recapture.** Print velho
prova o bug, nunca a correção.

**Quatro coisas INVALIDAM uma comparação visual, e nenhuma delas produz erro — a prova sai bonita e
é lixo. Confira as quatro ANTES da primeira captura:** (1) **tamanho/viewport diferente do que o
contrato fixou** — já se julgou uma referência de computador contra uma captura de celular;
(2) **idiomas diferentes nos dois lados** — o juiz compara `Save/Discard` com `Salvar/Descartar` e
julga tradução, não paridade, e isso já custou uma rodada inteira; (3) **elemento que termina
na borda do PNG é rolagem, não desenho** — não decide comparação; recapture rolado ou declare
não-comparado naquele ponto; (4) **o print enquadra a prova do ESTADO junto com o efeito** — print
que só significa junto de um comando fora dele vira disputa de palavra na revisão seguinte. Essas
quatro repetem no kick-off de toda Task visual (régua enterrada em contrato não alcança sessão que
nasceu depois dela — foi exatamente assim que a nº 2 custou a rodada).

**Toda afirmação sobre cor, sinal ou estado (`✓` / `✗` / `·`, habilitado, desabilitado) se escreve
com o detalhe AMPLIADO, nunca a olho na imagem inteira** — e a legenda cita a cor junto do sinal.
Numa Task de 38 prints, todo achado que sobreviveu à revisão veio de ampliar um detalhe que parecia
legível. Custo: um recorte a 300–400% por afirmação.

**Cada linha de legenda se escreve olhando aquele arquivo; "idem" é proibido.** Dois prints do mesmo
estado em larguras ou idiomas diferentes recebem duas descrições. Foi o template "idem / idem en /
idem mobile" que produziu **6 de 13 legendas erradas num trabalho em que os pixels estavam certos**.

**A prova de uma Task de comportamento termina no desfecho que o usuário pediu** — "conectou",
"salvou", "abriu" —, não no estado imediatamente anterior a ele. Print do botão habilitado não é
prova de que o clique funciona. Uma evidência que parou no botão desabilitado obrigou o portão a
rodar o fim do fluxo pra descobrir que o desfecho funcionava — uma rodada de revisão gasta com o
que a prova devia ter mostrado.

### 4. Olhe o print. Só delegue se você não conseguir

**Tente ler a imagem você mesmo primeiro**, pelo caminho absoluto. Muitos executores
enxergam — se você é um deles, olhe, responda as perguntas do passo seguinte e **acabou**:
delegar ali é só latência, e um intermediário a mais entre você e o pixel.

Delegue **apenas** quando a leitura falhar de fato — a ferramenta recusar o arquivo, um hook
bloquear, ou o modelo não aceitar imagem. Nesse caso, e só nesse:

1. comando de visão instalado nesta máquina, se houver (`see <imagem> "<pergunta>"` é o nome
   usual — confira com `command -v see`);
2. um subagente cujo modelo enxergue imagem, passando o **caminho absoluto**;
3. nenhum dos dois existindo, **diga ao árbitro antes de commitar** — quem tem visão no time
   (em geral o revisor) faz essa parte, e o combinado vai pro contrato.

Quando você **não** enxerga, o que chega até você é um caminho de arquivo, não um desenho:
descrever o print "pelo contexto", pelo nome do arquivo ou pelo que a conversa sugere é
**invenção**, por mais plausível que soe. E responder "não consigo ver imagens" também é
falso — consegue, por delegação. As duas saídas estão fechadas: ou você olha, ou alguém olha
por você.

### Tamanho não se olha: mede-se no DOM

Print responde **o que existe, onde, em que ordem**. Ele **não** responde tamanho, espaçamento nem
alinhamento — e é aí que quem julga por imagem erra com confiança.

Numa Task de tela o executor comparou o resultado com o mock por print, recebeu de volta "a
densidade está diferente", decidiu por argumento que o app real mandava, e commitou. O revisor mediu
a caixa: mock e aba irmã do mesmo painel em torno de 24px, entrega em 44px, em **sete** elementos.
Não era o app real ganhando: era uma altura mínima global comendo o CSS do componente, sem ninguém
sobrescrever. O print mostrava a diferença; só o número dizia de quem era a culpa.

Antes de decidir qualquer divergência de layout:

```js
// no navegador, com a tela aberta
[...document.querySelectorAll('.sua-classe')].map(e => e.getBoundingClientRect().height)
```

E meça **o vizinho real** — a aba irmã, a lista ao lado, o componente que já existe. "O app real
ganha" é uma regra sobre o app **medido**, não sobre o app imaginado.

Pergunta **específica**, nunca "está bom?" — vale tanto pra você olhando quanto pra quem
olha por você. Boas: *"o botão à direita do seletor tem moldura e a mesma altura dele, ou é
texto solto?"*, *"o item ativo se distingue dos outros?"*, *"algum retângulo opaco cobre o
fundo?"*, *"o texto cabe sem cortar nesta largura?"*. "Está bom?" devolve "está bom" e não
custa nada a ninguém.

### 5. Compare cego com a barra

O plano dá uma **barra** pra toda Task que mexe em pixel: uma tela nomeada, que dá pra abrir
e capturar, no mesmo estado e na mesma largura do seu print. Capture os dois lados e ponha um
**subagente fresco** pra escolher — sem dizer qual é qual:

> Duas imagens: `<dir-durável>/visual/A.png` e `<dir-durável>/visual/B.png`. Mesma tela, dois
> desenhos. **Qual das duas parece mais acabada?** Responda `A` ou `B`, depois o **maior
> buraco** da que perdeu, em uma frase concreta (o que está desalinhado, cortado, sem
> contraste, com altura diferente dos irmãos).

Três coisas que fazem isso valer alguma coisa:

- **Cego de verdade**: nome de arquivo neutro (`A`/`B`), e você **alterna** qual letra é o
  seu trabalho entre as rodadas. `novo.png` vs `referencia.png` não é cego — é uma dica.
- **Subagente fresco, nunca o braço que desenhou.** Quem construiu já sabe por que cada
  escolha foi feita, e defende ela. É o mesmo motivo de o revisor nunca ser a sessão que executou.
- **Escolha binária, não nota.** "Qual está melhor" tem resposta; "de 0 a 10, quanto está
  bom" devolve 7 sempre.

Perdeu → conserte **o maior buraco**, recapture, rode de novo. **Teto de 2 rodadas**, e daí
você commita com o resultado no reporte, mesmo perdendo. O teto não é preguiça: laço sem
fronteira de gasto é o modo de falha medido dessa técnica lá fora — gente torrando centenas
de dólares e jogando fora 95% do que saiu. Perdeu as duas rodadas → isso vai no reporte como
risco conhecido, e quem decide é o árbitro.

**O teto conta rodada de BARRA. Defeito de código não conta.** Rodada reprovada porque a tela está
quebrada — largura errada, foco preso, alvo de toque abaixo de 44px — não é sobre fidelidade e não
gasta o teto. Sem essa separação a Task estoura o teto com a tela quebrada, que é o oposto do que o
teto existe pra evitar. Uma Task fechou em quatro rodadas, **só a primeira de barra**; e outra
rodada reprovou por um recuo lateral empurrando o visor algumas centenas de pixels, que é bug de
largura real, não acabamento.

**E rodada que não toca pixel não paga barra de novo.** Commit de correção que só mexe em store,
teste ou backend não refaz comparação nenhuma — o `git show --stat` prova.

Você não enxerga imagem? O passo continua sendo seu — é o mesmo protocolo do passo 4: o
subagente de visão (ou o `see`) é quem olha, você é quem manda e quem lê a resposta.

**Contrato dizendo `Barra: nenhuma — decisão do usuário`: pule este passo inteiro** e commite
com os passos 1 a 4. Não invente uma barra por conta própria — a referência é escolha do
usuário, e uma escolhida por você mede o teu palpite, não o trabalho.

**Seu diff encosta em pixel e o contrato não traz nem barra nem dispensa? Pare e reporte ao
árbitro antes de commitar.** É decisão de fase 1 que ficou em branco; ele pergunta ao usuário
e te devolve a resposta. Commitar assim custa a Task inteira, porque o revisor devolve.

### O que vai no reporte

Por estado: caminho do print, o que você **clicou** e o que aconteceu, a pergunta que fez a
quem enxerga (se delegou) e o que voltou, e o que você mudou por causa disso.

Task com barra leva também: **quem venceu cada rodada cega** (e qual letra era a sua), o
maior buraco apontado, o que você consertou, e o caminho do print final. Perdeu no fim das
duas rodadas → diga isso na cara, com o buraco que sobrou.

Sem isso o revisor bloqueia a Task. Não é burocracia: é a única evidência que separa "o
código compila" de "a tela funciona", e as duas coisas já se descolaram aqui.
