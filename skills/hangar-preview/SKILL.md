---
name: hangar-preview
description: |
  Use sempre que a tarefa envolver VER, ler, clicar, testar ou tirar screenshot de uma
  página web, com ou sem o aviso "[hangar] ... navegador embutido" do hook e também em subagente — mexeu numa tela e quer conferir, "testa o login", "vê como ficou", "clica no botão
  lá", "lê o console da página" —, mesmo que o usuário não fale em preview ou navegador. A skill
  abre o navegador embutido desta sessão (`hangar-preview open URL`, ou a tool `browser_open` do
  MCP hangar) e o dirige por refs de acessibilidade. Sequência de várias ações numa tela que você
  ainda não conhece — navegar até uma tela, escolher item de lista, trocar combobox, preencher um
  cadastro inteiro — vai numa chamada só (verbo `objetivo`, o Jev decide cada passo), quando a
  sessão tem o Jev. Ela vence agent-browser e ver-front. Cada sessão tem o SEU navegador;
  --sessao opera o de outra só quando o usuário pedir. NÃO use para: quando o open respondeu que
  o app desktop está fechado (aí é agent-browser, avisando o usuário uma vez), gravar vídeo, vários
  navegadores ao mesmo tempo, navegador isolado, ou o túnel de porta do celular (PreviewSheet).
allowed-tools: Bash(hangar-preview:*)
---

# hangar-preview — dirigir o navegador embutido da sessão

Um Chromium de verdade por sessão: no app nativo, o WebView2 no Windows e um Chromium sem janela
no Linux; no Electron antigo, um view dele. O CLI acha sozinho o navegador da sua sessão (pela chave
do sidecar sem terminal, pelo tmux com terminal).

## MCP ou CLI

Com as tools do MCP `hangar` na sessão, use-as: são os mesmos verbos, sem Bash.

| Tool do MCP | CLI | Para |
|---|---|---|
| `browser_open {url}` | `open <url>` | abrir ou navegar; volta com o navegador pronto |
| `browser {verbo, args}` | `<verbo> <args>` | um verbo (`snapshot`, `click`, `goto`, `shot`...) |
| `browser_batch {passos}` | `batch` (stdin) | vários verbos já conhecidos; para no primeiro erro |
| `browser_goal {objetivo, dados}` | `objetivo <texto> --dados <json>` | o Jev cumpre um objetivo inteiro |
| `browser_check {estado}` | `confere <estado>` | o Jev diz se a tela já mostra o estado |

Pelo MCP, `erro:` volta como erro da tool e o `shot` devolve a imagem junto com o caminho.

## Qual leitura usar

- **`text`** — ler conteúdo (mensagem de erro, lista, resultado). Texto visível, com os iframes do
  mesmo site; o mais barato.
- **`snapshot --refs`** — achar onde clicar: só as linhas acionáveis, com ref e estado.
- **`snapshot`** — entender a estrutura da tela; corta em 24 mil caracteres com aviso (`--max`
  muda o teto).
- **`shot`** — ver layout, cor, alinhamento. Leia o PNG e **cite o caminho absoluto na resposta**
  (cru, sem crase): é assim que o app do celular mostra a imagem ao usuário.
- **`objetivo`** — várias ações numa tela que você não conhece. Uma chamada no lugar de
  `snapshot` → `click` → `snapshot`... (medido: ~20 comandos e 4 prints contra 2 e 1).
- **`batch`** — várias ações cujas refs você já tem.

## O fluxo comum: mexeu na tela, quer conferir

```
hangar-preview open http://localhost:3000/login   # volta com o navegador pronto e diz a URL
hangar-preview shot                               # ok: shot /home/.../.hangar/nav/shots/<sessao>/<ms>.png
hangar-preview close                              # terminou: fecha
```

Navegador já aberto? Troque de página com `goto <url>`, `back` ou `reload` — os três só voltam com
a página carregada. `open` de novo também serve, mas leva o painel à frente quando a sessão está
na tela do usuário.

O painel aparece na tela do usuário só quando ele abrir esta sessão no app. Diga que a página está
aberta no navegador da sessão, nunca que apareceu na tela dele.

## O ciclo: snapshot → alvo → ação

`click`, `fill` e `hover` recebem um **alvo**, de três jeitos:

- a **ref** (`@e1`, `@e2`...) do último `snapshot`: o caminho certo quando há vários parecidos;
- o **texto visível** do elemento (`click Salvar`, `fill "E-mail" ana@x.com`): sem snapshot antes.
  Casa primeiro o texto exato, depois o que contém; mais de um visível é recusa com a lista deles;
- um **seletor CSS** (`css=#extrato`, ou direto `#extrato`, `[aria-label=Buscar]`).

Não precisa de `eval` com `.click()` nem de `snapshot | grep` pra achar o botão. Só o que dá pra
acionar ganha ref; estados vêm depois dela:

```
$ hangar-preview snapshot --refs
- textbox "Email" [ref=@e3] [focused] [required] value="ana@x.com"
- checkbox "Lembrar" [ref=@e4] [unchecked]
- button "Entrar" [ref=@e5] [disabled]
$ hangar-preview click @e4
ok: click @e4
```

Estados: `[checked]`/`[unchecked]`/`[mixed]`, `[selected]`, `[pressed]`, `[expanded]`/`[collapsed]`,
`[disabled]`, `[focused]`, `[required]`, `[invalid]`, `value=...`.

**A ref morre em toda navegação e em todo re-render que desmonte o nó** (numa lista React, um
clique que muda a lista já invalida as refs dela). A resposta é sempre
`erro: ref @e5 nao existe (rode snapshot de novo)`: tire `snapshot` de novo e use as refs atuais.
`wait @eN` também refaz o snapshot por dentro e renumera as refs.

`click`, `fill` e `hover` rolam até o elemento antes de agir.

## Comandos

- `open <url>` — abre o navegador desta sessão (ou navega o que já existe) e só volta quando ele
  responde: o verbo seguinte já funciona. Nasce mesmo com a sessão fora da tela, medido em
  1280×800; quando o usuário abrir a sessão, a medida passa a ser a do painel.
- `snapshot [--refs] [--max <caracteres>]` — árvore de acessibilidade com refs e estados.
- `click <alvo>` — clique de mouse real. O `ok:` só sai com o evento entregue à página e a página
  assentada (o DOM parado, até 2 s); se ela navegou, a resposta diz para onde, já carregada.
  Evento não entregue: `erro: ... o evento nao chegou na pagina` — tire um `shot` e repita.
- `fill <alvo> <texto>` — clica no campo e substitui o conteúdo (eventos de teclado reais; React ok).
- `type <texto>` — digita no campo que já está com foco.
- `press <tecla>` — `Enter`, `Tab`, `Escape`, `ArrowDown`... Volta com a página assentada, como o
  `click`. Para voltar uma página use `back` (`Alt+ArrowLeft` não volta).
- `hover <alvo>` — passa o mouse (menus de hover).
- `goto <url>`, `back`, `reload` — navegam e esperam carregar; respondem `ok: <verbo> <url final>`.
- `wait <alvo> [--timeout <ms>]` — ver **Esperar**.
- `eval '<js>'` — estado que não é DOM (`localStorage`, variável global). Aceita expressão ou
  declarações (`const a=1; a+1`). Não use pra clicar, preencher ou navegar: há verbo pra isso. Se o
  js mexer no DOM, a resposta espera a página assentar.
- `text` — texto visível (`innerText`), com os iframes do mesmo site; iframe de outro site não dá
  pra ler, e a resposta termina dizendo quantos ficaram de fora.
- `url` — URL atual.
- `console [--limpar]` — `console.log/warn/error` (objetos com prévia: `{a: 1, b: "x"}`), erros de
  JS não tratados e promessas recusadas (com arquivo:linha), diálogos respondidos e downloads
  recusados.
- `dialog accept [texto] | dialog dismiss` — responde o `alert`/`confirm`/`prompt` que a página
  abriu (`texto` é a resposta do `prompt`). O diálogo fica aberto até alguém responder: o verbo que o
  abriu volta `ok: click — a pagina abriu um dialogo confirm: "...". Responda com: dialog accept |
  dialog dismiss`, e com ele aberto só `dialog`, `url`, `estado`, `console` e `network` funcionam (os
  outros voltam `erro: a pagina esta com um dialogo ... aberto`). Leia a pergunta antes de aceitar:
  `confirm` costuma ser "apagar?", "sair sem salvar?".
- `network` — `MÉTODO status url` e `MÉTODO FALHOU url: motivo`. A escuta liga na primeira chamada
  de `network` ou `wait --idle`: chame `network` uma vez ANTES da ação cujas requisições quer ver.
- `shot [arq.png]` — print da viewport; padrão `~/.hangar/nav/shots/<sessao>/<ms>.png`. Com
  `layout` numérico sai nas dimensões pedidas. Role com `eval 'scrollTo(0, 9999)'` para ver o fim.
- `folha` — junta em grade (até 6 por folha) os `shot` sem nome desde a última folha. **3 telas ou
  mais: leia a folha**, que custa o contexto de uma imagem. Precisa do ImageMagick.
- `tema <claro|escuro|sistema>` — emula `prefers-color-scheme`; vale através de navegações.
- `layout [mobile|desktop|<largura> <altura>]` — emula o viewport sem mexer na janela.
- `login [site]` — pede ao usuário, na tela dele, o login do Chrome dele para o site (padrão: o da
  página aberta; ex. `login github.com`). O painel mostra Permitir/Recusar e espera até 110 s.
  Responde `ok: <n> cookies para <site>` ou `erro: ...` (recusa e 0 cookies também são `erro:`); nunca devolve valor de
  cookie. Use quando a página pede login e você precisa entrar como o usuário. **Uma chamada por
  necessidade, nunca em laço**: cada uma é um pedido na tela dele.
- `estado` — `pausado` ou `livre`.
- `close` — fecha o navegador desta sessão. **Terminou, feche.** Deixe aberto só se ainda vai
  usar ou se o usuário vai testar a página, e diga isso na resposta.
- `list` — sessões com navegador vivo.
- `--sessao <nome>` — o navegador de OUTRA sessão, só quando o usuário pedir.

Respostas: `ok: ...` ou `erro: ...`; `snapshot`, `text`, `console` e `network` vêm em várias
linhas. O CLI sai com código 1 quando a resposta começa com `erro:` ou o transporte falhou.

## Pausa do usuário

O usuário pode pausar o navegador pelo painel enquanto o agente o dirige. Pausado, as ações
(`click`, `fill`, `type`, `press`, `hover`, `eval`, `layout`, `tema`, `goto`, `back`, `reload`, `dialog`) e o
`open` respondem `erro: o usuario pausou o navegador; espere ele clicar em Retomar no painel e
tente de novo`. A leitura (`snapshot`, `text`, `url`, `console`, `network`, `shot`, `wait`) segue
normal. Não insista em laço: termine o que dá pra ler, diga ao usuário que está esperando ele
retomar, e confira com `estado`.

## Esperar: `wait` com alvo, nunca `sleep`

`click`, `press`, `eval`, `goto`, `back` e `reload` já voltam com a página assentada: depois deles
não ponha `wait`. Espere só quando quer ver algo específico aparecer:

```
hangar-preview wait --idle              # rede parada e documento completo
hangar-preview wait --url login         # a URL passa a conter "login"
hangar-preview wait --text "bem-vindo"  # o texto aparece (sem diferenciar maiúsculas; inclui rótulo de botão)
hangar-preview wait @e3                 # a ref aparece (refaz o snapshot; refs mudam)
hangar-preview wait 800                 # ms fixo: só atrasa; quase nunca é o que você quer
```

Teto padrão de 15 s; `--timeout <ms>` muda, até 40 s. Estourar devolve
`erro: wait ... nao aconteceu em <ms>ms`. `--text` lê também os iframes do mesmo site; com iframe de
outro site na página, o erro diz quantos ficaram de fora (o texto pode estar lá: veja com `shot`).

`--idle` é zero requisição em voo, 500 ms sem resposta e `readyState === 'complete'`. A escuta de
rede liga no primeiro `--idle`: ele leva pelo menos 500 ms, e requisição já em voo antes dele não é
vista.

## Lote: `batch`

```
printf 'click @e2\nwait --idle\nfill @e5 usuario@example.com\nclick @e7\nwait --url dashboard\n' \
  | hangar-preview batch
```

Para na primeira linha que falhar. Linha vazia e `#` são ignoradas; `fill`, `type`, `eval` e `wait`
levam o resto da linha como um argumento só (sem aspas).

## Objetivo inteiro: `objetivo` e `confere` (Jev)

Precisa do Jev na sessão: `echo $HANGAR_JEV` diz `on`. Com `off` ou vazio, o verbo explica como
abrir uma sessão com ele (`hangar-send --new <nome> <cwd> --jev`, ou "Mais opções" ao criar); use
`batch` enquanto isso.

```
hangar-preview objetivo "cadastrar um novo serviço e salvar" \
  --dados '{"nome":"Exames laboratoriais","cnae":"8640201","aliquota":"3"}' --passos 22
```

O Jev decide a operação e o alvo a cada passo e vai até o fim sozinho (campo de texto, `<select>`,
dropdown do Radix, switch, combobox com autocomplete). Ele acha o elemento pelo rótulo: não precisa
de `snapshot` antes.

- `--dados` leva os valores que você já sabe; as chaves são livres (o Jev casa com o rótulo).
- A saída: uma linha por passo (`jev-objetivo passo N: ...`), depois `parou: <motivo>`,
  `url: <url final>`, `texto: <começo do texto da página>`, `falta-dado: <campo>` quando faltou um
  valor, e `acoes: N`. Sai 0 só quando cumpriu.
- `falta-dado:` → chame de novo com esse valor em `--dados`; a página fica onde parou.
- Confira o resultado lendo a tela (`text`, `shot`), não pelo log.

`confere "<estado>"` responde `confere: 0.NN` e sai 0 = sim, 1 = ainda não, 2 = falhou. Use antes
do `shot` ou para saber se uma ação deu certo.

## Se um comando falhar

**Falha não autoriza trocar de navegador.** Não abra outro navegador nem use agent-browser,
Playwright, ferramentas do Chrome ou cliques por JavaScript para contornar. A exceção é o app
desktop fechado: aí a resposta diz que repetir não adianta; avise o usuário uma vez e siga com
agent-browser.

1. Leia o erro e confira se a ação anterior já aconteceu antes de repeti-la (envio de formulário
   principalmente). `ok: click` não prova que o botão fez o que devia: confirme com `wait`.
2. Tire outro `snapshot` e use refs novas; página navegando ou montando, espere com `wait`.
3. Clique, teclado ou preenchimento sem efeito: confira foco, visibilidade e carregamento com
   `eval '({visible:document.visibilityState,focus:document.hasFocus(),active:document.activeElement?.tagName})'`
   e olhe o `console`. Não afirme a causa sem conferir.
4. Tente de novo depois de verificar a condição. Persistindo, diga ao usuário o erro exato e o que
   conferiu.

Se o app desktop fechar ou reiniciar (a atualização reinicia), o CLI responde em uma frase: tente
uma vez depois de 15 s; falhando de novo, não repita o `open` em laço.

## O que NÃO alcança

- **`snapshot` e alvos dentro de `iframe`.** O `snapshot` e os alvos só enxergam o frame principal;
  `text` e `wait --text` leem iframes do mesmo site e só contam os de outro site (Memed, checkout de
  terceiro). Se o que você precisa está num iframe de outro site, use `shot` e avise o usuário que
  a leitura e o clique ali estão fora do alcance; não tente furar com `eval`.
- **Electron antigo**: tem até 8 abas (`tab list|new <url>|<id>|close [id]`, `--aba <id>` age numa
  aba sem trocar a ativa); no app nativo elas respondem `erro: o app nativo ainda nao tem abas`.
  No Electron, aba que nasce fora da tela pode vir com `innerWidth` 0: use `layout 1280 800`.
