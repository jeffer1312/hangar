# Manual

Cada arquivo aqui é um artigo da tela **Manual** do app nativo e do Expo. Quem lê os arquivos é a
rota `GET /api/manual`. Este README não é artigo e não aparece na tela.

## Formato

```markdown
---
titulo: Conectar o celular
categoria: comecar
resumo: Pelo Hangar Connect ou pelo Tailscale.
icone: smartphone
ordem: 10
onde: todos
ancora: new-session
---
Texto curto antes da primeira seção (opcional).

## Primeira seção
texto…

## Segunda seção
…
```

O nome do arquivo é o id do artigo: só `a-z`, `0-9` e `-` (ex.: `conectar-celular.md`). O texto antes
do primeiro `## ` é a abertura.

- Título de seção é `## ` no começo da linha, sem recuo, com um espaço depois. Cada um vira uma
  seção e uma linha do índice "Nesta página".
- `###` é subtítulo dentro da seção e não aparece em "Nesta página".
- Títulos de seção não se repetem no mesmo artigo.

| Campo | Para que serve |
|---|---|
| `titulo` | Obrigatório. Sem ele o artigo fica fora. |
| `categoria` | Obrigatório. Uma das quatro abaixo; outra fica fora. |
| `resumo` | Uma frase que aparece na lista de artigos. |
| `icone` | Nome de um ícone Lucide em kebab-case (`smartphone`, `key-round`). |
| `ordem` | Número da posição dentro da categoria (menor primeiro). Use dezenas para caber artigo novo no meio. |
| `onde` | `desktop`, `celular` ou `todos`: em que app o artigo aparece. |
| `ancora` | Opcional. Id de uma peça do app nativo (lista em `desktop-native/src/tour/anchors.rs`); vira "Mostrar no app". Use só peça que está na tela sem abrir nada antes. |

Categorias, na ordem das abas:

| `categoria` | O que entra |
|---|---|
| `comecar` | Conectar, abrir a primeira conversa, contas. |
| `sessoes` | Trabalhar com várias conversas, organizar a lista, agendar, navegador. |
| `ferramentas` | Cofre, ditado, voz e o que mais se usa dentro da conversa. |
| `problemas` | Conversa parada, que caiu, que não responde. |

Uma novidade com `manual: <id>` (em `docs/novidades/`) aparece no bloco "Mudou recentemente" do
artigo e ganha o botão "Ler no manual".

## Como escrever

- **Um artigo por tarefa de quem usa o app**: "conectar o celular", "guardar uma senha". Não um
  artigo por tela nem por recurso interno.
- **Frases curtas, em português do dia a dia.** Escreva para quem nunca abriu o código. Diga o que
  a pessoa faz e o que acontece.
- **Caminho de tela no formato `Configurações › Connect`**, com os nomes exatamente como aparecem
  no app (confira em `messages/pt.json` e na tela). Botão em **negrito**: clique em **Nova tarefa**.
- **Só passo que existe na tela.** Na dúvida, deixe o passo fora.
- **Quando o caminho muda entre computador e celular, diga os dois**: "No computador, … No celular, …".
- **Sem nome de arquivo, função, rota, variável ou comando de terminal.** Atalho de teclado pode
  (Ctrl+K).
- **Sem data nem versão no texto.** O que mudou e quando fica nas novidades.
- Seção curta, de 2 a 6 frases ou uma lista de passos. Artigo de tarefa com mais de seis seções
  provavelmente são dois artigos; artigo de problemas pode ter uma seção por sintoma.
