---
name: hangar-preview
description: |
  Use quando a sessão tem um navegador embutido aberto no app Hangar (o painel "Navegador" ao lado
  do chat, só no app desktop Electron) e a tarefa pede pra VER, ler, clicar, testar ou tirar
  screenshot da página que está nele — "vê o que tem na minha página", "testa o fluxo no preview",
  "clica no botão lá", "tira um print do preview", "lê o console da página". Cada sessão tem o SEU
  navegador; esta skill é o jeito de dirigir o da sua (ou, com --sessao, o de outra sessão quando o
  usuário pedir). NÃO use para: sites fora do Hangar (isso é o agent-browser), o túnel de porta do
  celular (PreviewSheet), ou abrir navegador pra uma sessão que não tem — o painel é aberto pelo
  usuário, nunca pelo agente.
allowed-tools: Bash(hangar-preview:*)
---

# hangar-preview — dirigir o navegador embutido da sessão

O navegador embutido é um Chromium de verdade (view nativo do Electron), um por sessão. O CLI
resolve sozinho QUAL é o da sua sessão (pelo nome da sessão tmux) e fala com o servidor local do
shell — duas sessões com a mesma URL aberta não se confundem.

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

- `hangar-preview open <url>` — abre o navegador desta sessão com a url (o painel monta sozinho na
  tela do usuário; se a sessão estiver fora da tela, abre quando ele abrir ela). **Avise o usuário**
  no texto da resposta que você abriu — a janela dele muda na hora.
- `hangar-preview snapshot` — árvore de acessibilidade compacta, com as refs atuais.
- `hangar-preview click @eN` — clica (evento de mouse real, não `.click()` em JS).
- `hangar-preview fill @eN <texto>` — foca o campo e substitui o conteúdo pelo texto.
- `hangar-preview type <texto>` — digita no elemento que já está com foco (sem focar nada antes).
- `hangar-preview press <tecla>` — uma tecla: `Enter`, `Tab`, `Escape`, `ArrowDown`...
- `hangar-preview hover @eN` — passa o mouse sobre a ref (menus que só abrem no hover).
- `hangar-preview wait <alvo>` — espera algo acontecer; ver seção **Esperar** abaixo.
- `hangar-preview eval '<js>'` — roda JS na página e imprime o resultado (`ok: <json>` ou `erro:
  <mensagem>`). Use pra ler estado que não é DOM (`localStorage`, uma variável global) ou pra algo
  que os verbos acima não cobrem — não pra clicar/preencher, que têm verbo próprio.
- `hangar-preview tema <claro|escuro|sistema>` — emula `prefers-color-scheme` **nesta sessão**. Fica
  valendo através de navegações (o Electron perde a emulação ao navegar; o controlador reaplica
  sozinho). `sistema` volta ao tema real da máquina.
- `hangar-preview console [--limpar]` — log do console da página (`console.log`/`warn`/`error` e
  erros de runtime). `--limpar` esvazia o buffer depois de ler.
- `hangar-preview network` — últimas respostas de rede (status + URL).
- `hangar-preview url` — URL atual.
- `hangar-preview shot [arq.png]` — screenshot; default `/tmp/hangar-preview-<sessao>.png`. Leia o
  PNG com a ferramenta de leitura de imagem.
- `hangar-preview list` — quais sessões têm navegador vivo agora.
- `--sessao <nome>` opera o navegador de OUTRA sessão — só quando o usuário pedir, e avise-o.

Toda resposta é uma linha só: `ok: ...` ou `erro: ...`. Leia o TEXTO pra saber se deu certo — o
código de saída do processo só reflete falha de transporte (sessão sem navegador, servidor fora do
ar), não falha do comando em si (`erro: ref @eN nao existe` sai com código 0, porque é uma resposta
válida do navegador, não uma falha de chamada).

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
desse fetch terminar, que é o ponto que importa pro agente.

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

## Regras

- O alvo é **sempre o navegador da própria sessão** — resolvido pelo nome da sessão tmux, sem
  precisar de flag. `--sessao <nome>` é a exceção explícita, só quando o usuário pedir pra mexer no
  navegador de outra sessão.
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
