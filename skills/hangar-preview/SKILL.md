---
name: hangar-preview
description: |
  Use sempre que o app desktop do Hangar estiver aberto (o hook avisa "[hangar] ... navegador
  embutido" no prompt) e a tarefa envolver VER, ler, clicar, testar ou tirar screenshot de uma
  página web — mexeu numa tela e quer conferir, "testa o login", "vê como ficou", "clica no botão
  lá", "lê o console da página" —, mesmo que o usuário não fale em preview ou navegador. A skill
  ABRE o navegador embutido desta sessão (`hangar-preview open URL`, o painel monta na tela do
  usuário) e o dirige por refs de acessibilidade. Qualquer sequência de mais de uma ação — navegar
  até uma tela, escolher item de lista, trocar combobox, preencher um cadastro inteiro — vai numa
  chamada só (verbo `objetivo`, um laço em que o modelo Jev decide cada passo, sem ida ao
  modelo grande entre eles), nunca clique por clique. Com o app desktop aberto
  ela vence agent-browser e ver-front pra página local. Cada sessão tem o SEU navegador; --sessao opera o de outra só
  quando o usuário pedir. NÃO use para: máquina sem o app desktop (sem o aviso do hook, é
  agent-browser), site externo que precisa do login do usuário (browser-harness), ou o túnel de
  porta do celular (PreviewSheet).
allowed-tools: Bash(hangar-preview:*)
---

# hangar-preview — dirigir o navegador embutido da sessão

O navegador embutido é um Chromium de verdade (view nativo do Electron), um por sessão. O CLI
resolve sozinho QUAL é o da sua sessão: pela chave estável do sidecar no modo sem terminal e pelo
nome do tmux no modo com terminal. Depois fala com o servidor local do shell — duas sessões com a
mesma URL aberta não se confundem.

## O fluxo comum: mexeu na tela, quer conferir

O hook diz no prompt se o app desktop está aberto e, se esta sessão já tem navegador, em qual URL.
Daí:

```
hangar-preview open http://localhost:3000/login    # só se ainda não houver navegador (pisca na tela dele)
hangar-preview wait --idle                          # rede parada
hangar-preview shot /tmp/login-01.png               # print da viewport
hangar-preview close                                # terminou: o painel some da tela do usuário
```

Leia o PNG com a ferramenta de leitura de imagem e **cite o caminho absoluto do arquivo no texto da
resposta** (`/tmp/login-01.png`, cru, sem crase) — é assim que o app do celular mostra a imagem pro
usuário. Print tirado e não citado é print que ele não vê.

**Navegador já aberto em outra rota? Não chame `open` de novo** — cada `open` remonta o painel na
tela do usuário. Troque de rota por dentro e espere:

```
hangar-preview eval 'location.href="/conversa"'
hangar-preview wait --idle
```

## Mais de UMA ação na página? Então NÃO use o ciclo abaixo

Qualquer sequência — preencher um cadastro, mas também abrir uma folha, escolher um item da lista,
trocar um combobox, chegar até a tela que você quer conferir — é **uma chamada só**:

```
hangar-preview objetivo "cadastrar um novo serviço e salvar" \
  --dados '{"nome":"Exames laboratoriais","cnae":"8640201","aliquota":"3"}'

hangar-preview objetivo "escolher a pasta hangar na lista, trocar a conta para claude-200-5 e abrir o dropdown Modelo"
```

O Jev decide cada passo e vai até o fim sozinho, combobox e autocomplete incluídos. Ele acha o
elemento pelo RÓTULO — você não precisa de `snapshot` pra descobrir a ref. Encadear
`snapshot` → grep da ref → `click` → `snapshot` de novo põe a árvore inteira no seu contexto a
cada volta; medido na mesma verificação de tela, ~20 comandos e 4 prints contra 2 comandos e 1.

Depois do `objetivo`, o `shot` é seu: ele responde "objetivo atingido", não devolve o conteúdo da
tela. Detalhes, flags e o que fazer quando ele para: **Preencher um formulário inteiro**, abaixo.

O ciclo a seguir é pro resto: UM clique solto, ler console, tirar print.

## O ciclo: snapshot → @ref → ação

Não existe seletor CSS nem XPath. Toda ação em elemento usa uma **ref** (`@e1`, `@e2`, ...) tirada
de um `snapshot` — a árvore de acessibilidade compacta, só com o que tem papel útil, numerada só no
que dá pra clicar/preencher.

```
$ hangar-preview snapshot
- RootWebArea "Example Domain"
  - heading "Example Domain"
  - paragraph
    - link "Learn more" [ref=@e1]

$ hangar-preview click @e1
ok: click @e1
```

Página grande enche o contexto com `StaticText` e `paragraph`. Quando você só quer agir, filtre:
`hangar-preview snapshot | grep 'ref='`. Quando só quer LER (mensagem de erro, lista, resultado),
use `hangar-preview text` — o texto visível da página, sem árvore nenhuma.

`click`, `fill` e `hover` rolam a página até o elemento antes de agir: ref abaixo da dobra
funciona sem `scrollIntoView` por `eval`.

**A ref morre em toda navegação e em todo re-render que desmonte o nó.** Não é só trocar de
página: numa lista React, um clique que faz a lista mudar já invalida as refs seguintes daquela
lista, mesmo sem sair da URL. A resposta do CLI nesse caso é sempre a mesma linha:

```
erro: ref @e5 nao existe (rode snapshot de novo)
```

A saída é **tirar `snapshot` de novo e reler as refs atuais** — nunca insistir na mesma ref, nunca
tentar de novo sem antes reler a árvore. Ref velha e ref que nunca existiu dão o mesmo erro de
propósito: pro agente o tratamento é idêntico.

**`wait @eN` também REFAZ o snapshot por dentro** (é assim que ele confirma que o elemento apareceu)
— e por isso RENUMERA as refs. Uma ref guardada de antes de um `wait` não é confiável depois dele;
tire `snapshot` de novo antes de agir.

## Comandos

- `hangar-preview open <url>` — abre o navegador desta sessão com a url. Com o app desktop aberto
  ele nasce em segundos **mesmo com a sessão fora da tela** (escondido; `list` mostra `vivo`), e
  todos os verbos funcionam ali, `shot` incluído: a página escondida é medida em 1280×800, então
  o que você lê e fotografa é o layout de desktop. Quando o usuário abrir a sessão, o painel
  aparece com o navegador na página em que você deixou, e aí a medida passa a ser o tamanho real
  do painel — um `shot` de antes e um de depois podem ter tamanhos diferentes.
  **Avise o usuário** no texto da resposta que você abriu — a janela dele muda quando ele for lá.
  **Se `innerWidth` vier 0**, a medida de 1280×800 não foi ligada — acontece com a aba que NASCE
  com a sessão fora da tela. Sem medida, `snapshot` e `shot` saem de uma página sem layout. A
  saída é `layout 1280 800`; `layout desktop` NÃO serve aqui, porque limpa a emulação e devolve
  o tamanho real, que é zero. Confira com `eval 'innerWidth'` antes de concluir que a página
  está vazia.
- `hangar-preview snapshot` — árvore de acessibilidade compacta, com as refs atuais.
- `hangar-preview click @eN` — clica (evento de mouse real, não `.click()` em JS). O `ok:` agora
  só sai com o evento comprovadamente entregue: a aba escondida que acabou de navegar para de
  compor quadro e o Chromium engole o clique, e nesse caso o verbo reancora o quadro, tenta uma
  vez e responde `erro: ... o evento nao chegou na pagina`. Vendo esse erro, um `shot` também
  ressuscita. Medição em `docs/decisoes/frontend.md`.
- `hangar-preview fill @eN <texto>` — foca o campo e substitui o conteúdo pelo texto.
- `hangar-preview type <texto>` — digita no elemento que já está com foco (sem focar nada antes).
- `hangar-preview press <tecla>` — uma tecla: `Enter`, `Tab`, `Escape`, `ArrowDown`...
- `hangar-preview hover @eN` — passa o mouse sobre a ref (menus que só abrem no hover).
- `hangar-preview wait <alvo>` — espera algo acontecer; ver seção **Esperar** abaixo.
- `hangar-preview eval '<js>'` — roda JS na página e imprime o resultado (`ok: <json>` ou `erro:
  <mensagem>`). Use pra ler estado que não é DOM (`localStorage`, uma variável global) ou pra algo
  que os verbos acima não cobrem — não pra clicar/preencher, que têm verbo próprio. Aceita
  expressão (`document.title`) e também declarações, como o console do DevTools: em
  `location.reload(); "ok"` ou `const a=1; a+1` o resultado é o valor da última expressão, sem
  precisar de `return`.
- `hangar-preview tema <claro|escuro|sistema>` — emula `prefers-color-scheme` **nesta sessão**. Fica
  valendo através de navegações (o Electron perde a emulação ao navegar; o controlador reaplica
  sozinho). `sistema` volta ao tema real da máquina.
- `hangar-preview layout [mobile|desktop|<largura> <altura>]` — sem valor, informa o layout atual.
  `mobile` e `desktop` são atalhos; dois inteiros positivos, como `1366 768`, emulam exatamente
  esse viewport via CDP sem redimensionar a janela do Hangar. O tamanho continua valendo após
  navegação, `open` e troca de aba.
- `hangar-preview console [--limpar]` — log do console da página (`console.log`/`warn`/`error` e
  erros de runtime). `--limpar` esvazia o buffer depois de ler.
- `hangar-preview network` — últimas respostas de rede (status + URL). A escuta de rede só liga na
  **primeira** chamada de `network` ou de `wait --idle` (custa CPU o tempo todo, então não fica
  ligada à toa): a primeira leitura devolve pouco ou nada, e o buffer só enxerga daí pra frente.
  Quer ver as requisições de um clique? Chame `network` uma vez ANTES do clique.
- `hangar-preview text` — texto visível da página (`innerText`), cru. Pra ler conteúdo sem pagar
  a árvore do `snapshot`.
- `hangar-preview url` — URL atual.
- `hangar-preview shot [arq.png]` — screenshot da **viewport** (o que está na tela, não a página
  inteira; com layout numérico, o PNG sai nas dimensões pedidas, não nas do painel; role com
  `eval 'scrollTo(0, 9999)'` pra ver o fim); default
  `/tmp/hangar-preview-<sessao>/<hora>.png`, um arquivo por shot. Leia o PNG com a ferramenta de
  leitura de imagem e cite o caminho na resposta.
- `hangar-preview folha` — junta numa grade (até 6 telas por folha, numeradas na ordem) os `shot`
  sem nome tirados desde a última folha e imprime o caminho de cada folha. **Vai conferir 3 telas
  ou mais? Tire os shots e leia a folha, não cada PNG**: uma folha custa o contexto de uma imagem.
  Serve pra layout e visão geral; texto miúdo, cor exata ou habilitado/desabilitado → leia só
  aquele shot em tamanho cheio. Precisa do ImageMagick (`magick`).
- `hangar-preview close` — fecha o navegador desta sessão de verdade (o painel some da tela do
  usuário, o view morre). **Terminou de usar, feche.** Deixe aberto só se você ainda vai dirigir
  ele ou se o usuário vai testar a página — e nesse caso diga isso na resposta. Navegador aberto
  numa página cujo servidor você já derrubou é lixo órfão na tela dele.
- `hangar-preview objetivo <texto> [--dados <json>] [--passos N]` — preenche um formulário inteiro
  numa chamada só, com o Jev decidindo cada passo; ver **Preencher um formulário inteiro** abaixo.
- `hangar-preview confere "<estado>"` — o Jev diz se a tela já mostra o estado (sai 0 = sim,
  1 = ainda não, 2 = falhou); use antes do `shot`.
- `hangar-preview list` — quais sessões têm navegador vivo agora.
- `--sessao <nome>` opera o navegador de OUTRA sessão — só quando o usuário pedir, e avise-o.

Toda resposta é uma linha só: `ok: ...` ou `erro: ...`. Leia o TEXTO pra saber se deu certo — o
código de saída do processo só reflete falha de transporte (sessão sem navegador, servidor fora do
ar), não falha do comando em si (`erro: ref @eN nao existe` sai com código 0, porque é uma resposta
válida do navegador, não uma falha de chamada).

## Abas

O navegador de cada sessão tem até 8 abas. A **ativa é uma só**, compartilhada entre o painel do
usuário e o CLI: clicar numa aba no painel muda onde os próximos comandos caem.

| Comando | Faz |
|---|---|
| `hangar-preview tab list` | Lista as abas; `*` marca a ativa. |
| `hangar-preview tab new <url>` | Abre outra aba e a torna ativa. |
| `hangar-preview tab <id>` | Torna essa aba a ativa (é a que o usuário vê). |
| `hangar-preview tab close [id]` | Fecha a aba (sem id, a ativa). A última fecha o navegador. |

`--aba <id>` age em outra aba **sem trocar a ativa** — vale para `snapshot`, `click`, `fill`,
`type`, `press`, `hover`, `wait`, `eval`, `console`, `network`, `text`, `url`, `shot`. O print de
aba escondida custa 2-3 s a mais que o da visível.

`url` e `shot` terminam em ` (aba N de M)` quando a sessão tem mais de uma aba — é assim que se
sabe onde se está sem pedir `tab list`. Com uma aba só a saída é a mesma de sempre.

No `batch` cada linha pode levar o seu `--aba`, e linhas `tab ...` valem normalmente.

## Se um comando falhar

**Falha não autoriza trocar de navegador.** Continue pelo `hangar-preview`: não abra outro
navegador nem use agent-browser, Playwright, ferramentas do Chrome ou cliques por JavaScript
para contornar o problema. A falha pode ser temporária; investigue e tente novamente.

1. Leia o erro e confira se a ação anterior já aconteceu antes de repeti-la, principalmente
   envio de formulário. `ok: click` sozinho não prova que o botão executou a ação: confirme com `wait`.
2. Tire outro `snapshot` e use referências novas. Se a página estiver navegando ou montando,
   espere uma condição com `wait` antes de repetir.
3. Se clique, teclado ou preenchimento não funcionarem, confira foco, visibilidade e carregamento.
   Para esse diagnóstico, `eval '({visible:document.visibilityState,focus:document.hasFocus(),active:document.activeElement?.tagName})'`
   lê o estado sem simular uma interação. Conseguir ler ou tirar print não prova que a página
   está recebendo os cliques e as teclas. Não afirme a causa sem conferir.
4. Depois de verificar a condição, tente a ação novamente e confirme o resultado. Não repita
   às cegas. Se persistir, informe ao usuário o erro exato e o que foi verificado e ajude a
   resolver o problema no navegador embutido; não abandone a ferramenta nem escolha outra por conta própria.

## Esperar: `wait`, nunca `sleep`

Depois de clicar ou navegar, espere a página responder com `wait` — nunca `sleep`/`timeout` fixo.
Um `sleep` curto flaqueia em página lenta e um `sleep` longo desperdiça o turno numa página rápida.

```
hangar-preview wait --idle              # rede parada
hangar-preview wait --url login         # a URL passa a conter "login"
hangar-preview wait --text "Bem-vindo"  # o texto aparece em algum lugar da página
hangar-preview wait @e3                 # a ref @e3 aparece (refaz snapshot; refs mudam depois)
hangar-preview wait 800                 # ms fixo — só quando nada acima serve
```

`--idle` **não** é "a página respondeu" — é rede parada de verdade: zero requisição em voo, mais
500ms de silêncio depois da última resposta, mais `document.readyState === 'complete'`. Uma SPA que
dispara um fetch logo após a navegação (dashboard carregando dados) só passa no `--idle` depois
desse fetch terminar, que é o ponto que importa pro agente. A escuta de rede liga no primeiro
`--idle` (ou `network`) da sessão, e o silêncio conta a partir dali — o primeiro `--idle` leva pelo
menos 500ms mesmo em página parada, e uma requisição que já estava em voo antes dele não é vista.

Todo `wait` tem teto (15s por padrão); estourar devolve `erro: wait ... nao aconteceu em 15000ms`
em vez de travar o comando pra sempre.

## Lote: várias ações num turno

`batch` lê comandos do stdin, um por linha, e roda em sequência — é o jeito padrão de fazer uma
sequência de passos sem gastar uma chamada de ferramenta por linha:

```
printf 'click @e2\nwait --idle\nfill @e5 usuario@example.com\nclick @e7\nwait --url dashboard\n' \
  | hangar-preview batch
```

Linha em branco e linha começando com `#` são ignoradas. Texto com espaço não precisa de aspas
(`fill @e5 nome completo aqui` funciona) — só `fill`, `type`, `eval` e `wait` levam o resto da linha
como um único argumento de texto.

## Preencher um formulário inteiro: `objetivo`

```
hangar-preview objetivo "cadastrar um novo serviço e salvar" \
  --dados '{"nome":"Exames laboratoriais","cnae":"8640201","aliquota":"3"}' --passos 22
```

Um laço em que o modelo Jev decide a operação e o alvo a cada ciclo. **Você chama uma vez e ele
vai até o fim** — não há ida ao modelo grande entre os passos, então o custo não cresce com o
número de campos. Prefira-o a conduzir na mão sempre que forem vários campos, ou um formulário
que você não conhece.

Ele resolve sozinho: campo de texto, `<select>` nativo, dropdown do Radix, switch, e **combobox
editável** (autocomplete — abre, digita, espera a lista aparecer e escolhe a opção).

- `--dados` leva os valores que VOCÊ já sabe, em JSON. As chaves são livres: o Jev casa a chave
  com o rótulo da tela, então mande o valor real (`"plano":"Unimed"`), não só o nome do campo.
- Campo sem valor em `--dados` vai a um modelo de texto pequeno. Não sabendo o dado, o laço para
  e imprime `falta-dado: <campo>` — chame de novo com esse dado em `--dados`, que a página fica
  onde parou e ele continua dali.
- Ele para sozinho quando a página confirma que acabou. Qualquer outra parada vem com o motivo na
  linha `parou:` (alvo sem confiança, alvo repetido, operação arriscada, erro do navegador).
- **Confira o resultado lendo a tela** (`text` ou `snapshot`), nunca pelo log do laço.

Exige a chave do Jev na sessão: nasça com `hangar-send --new <nome> <cwd> --jev`, ou marque o Jev
em "Mais opções" ao criar. Sem ela o verbo diz onde cadastrar.

## Regras

- O alvo é **sempre o navegador da própria sessão** — resolvido pelo sidecar no modo sem terminal
  ou pelo tmux no modo com terminal, sem precisar de flag. `--sessao <nome>` é a exceção explícita,
  só quando o usuário pedir pra mexer no navegador de outra sessão.
- O view continua vivo quando o usuário troca de sessão no app — você pode seguir trabalhando nele
  via CLI em background, sem atrapalhar a tela dele.
- Não fique abrindo e fechando navegador em loop nem trocando a url a cada passo: cada `open` pisca
  na tela do usuário. Abra uma vez, trabalhe com os outros comandos.
- Nota de rodapé pra quem for curioso: `fill` funciona em campo de formulário comum de framework
  (React incluso) porque usa evento de teclado real, não `value=` em JS — não precisa fazer nada
  especial pra isso, é o comportamento padrão do verbo.

## O que NÃO alcança

- **Conteúdo dentro de `iframe` fica fora do alcance.** A árvore de acessibilidade do `snapshot` só
  enxerga o frame principal da página — um `iframe` (mesmo de mesma origem) não aparece nela, e um
  `iframe` de outra origem é um alvo CDP separado que este CLI não segue. Se a página usa iframe pra
  embutir a área que você precisa mexer (editor de texto rico, player, checkout de terceiro), o
  sintoma é o `snapshot` simplesmente não listar o elemento — parece "não achei o elemento" e engana
  fácil pra procurar mais fundo na mesma árvore. Não tem contorno hoje: não tente `eval` pra furar o
  iframe nem insista tirando snapshot de novo — avise o usuário que aquele trecho está fora do
  alcance do preview.
- Página com árvore de acessibilidade muito grande (milhares de nós) deixa o `snapshot` pesado —
  puxe menos JS na página ou navegue pra uma rota mais específica antes.
