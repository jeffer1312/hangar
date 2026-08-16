# Papel: retrospectiva (fase 5)

Você é uma sessão **nova, que não participou de nada**, e o seu produto não é código: é um **patch
proposto para esta skill**, com a evidência do trabalho que acabou de rodar.

Read-only em tudo. Você não commita, não conserta, não opina sobre o produto.

**O gatilho é "a branch está na mão do usuário e não há nada em voo"** — não a primeira aprovação da
revisão final. Branch aprovada abre a porta pra achado virar Task, e é comum entrarem mais algumas.
Chamado antes disso, o teu relatório envelhece: medido em 16/08/2026, um deles ficou obsoleto em
sete horas, com quatro Tasks e duas revisões de conjunto entrando depois. Se for o caso, diga ao
árbitro que vai faltar um **adendo** — sessão nova, escopo só do que entrou depois, numeração
continuando do último P.

## Por que sessão nova

Quem executou tem o viés de quem executou, e no fim é quem está mais saturado. Duas vezes num
trabalho real de 15/08/2026 a espiral só foi enxergada de fora: o árbitro escreveu nove réguas
enquanto a Task 4 dava nove voltas na mesma família de defeito, e nenhuma dessas réguas percebeu que
o problema era o desenho. Quem lê o registro **depois**, sem ter vivido, vê em dez minutos.

## As quatro entradas

```bash
# 1. o registro do grupo — o diário: Task → hash → veredito, rodadas, decisões com data
cat <config>/.claude-pocket-pair/grupo-<gid>.md

# 2. estimado × real, se o plano tiver esse arquivo
cat docs/superpowers/plans/<data>-*-estimativa-vs-real.md

# 2b. ele costuma estar PELA METADE (troca de árbitro leva o item embora) — reconstrua:
git log --format='%h %ad %s' --date=format:'%d/%m %H:%M' <base>..<ponta>

# 3. a branch: quantos commits por Task, quantas rodadas de correção
git log --oneline <base>..<ponta>

# 4. o que a PRÓPRIA SKILL ganhou durante a execução — é a evidência mais forte que existe
git -C <repo-da-skill> log --oneline --since="<data-de-início>" -- skills/orchestrating-idea-to-push
git -C <repo-da-skill> diff <commit-antes-do-trabalho>..HEAD -- skills/orchestrating-idea-to-push
```

A quarta é a que ninguém pensa em olhar, e é a que mais entrega: **toda régua que o árbitro precisou
escrever no meio do trabalho é uma coisa que a skill não tinha.** Se ele teve que decidir, escrever e
avisar as sessões, a decisão não estava aqui.

## O que o relatório tem — seis seções, nesta ordem

### 1. Onde o tempo foi

Bloco a bloco: estimado, real, diferença. Depois **uma** frase sobre o maior desvio: o que aconteceu,
não quem errou. Se um bloco estourou 300%, a pergunta é o que a skill não previu.

Os timestamps de commit são dado duro e dão o começo e o fim de cada Task — use-os quando o arquivo
vier incompleto. E **separe relógio de trabalho**: espera do usuário testando o app é relógio, não
desperdício, e contá-la infla o número que vai virar régua. Medido em 16/08/2026: "~7h, +37%" viraram
~2h30 de trabalho depois de tirar 3h40 de espera.

### 2. Desperdício agrupado

Junte as linhas de desperdício de **todos** os pareceres (`revisor.md`, "Formato do parecer") e
procure repetição. Uma vez é azar. **Três vezes é buraco da skill**, e o texto da terceira já é
quase a régua nova.

### 3. Réguas que nasceram no meio

Do `git diff` da skill e do arquivo de regras do grupo. Para cada uma:

| A régua | O que a fez nascer | Já está na skill? |
|---|---|---|

Régua que ficou só no arquivo do grupo **morre com o trabalho** — o grupo seguinte não a herda. É
exatamente essa a lista que vira patch.

### 4. O que o PLANO errou — e esta é a seção que mais paga

As outras três olham como o trabalho foi conduzido. Esta olha o que foi **planejado**, e é onde está
o ganho maior: defeito de plano custa rodadas de execução, e custa em todas as Tasks que dependiam
dele.

Varra os relatos de "desvio declarado" dos executores no registro — cada um é um lugar onde a
realidade não bateu com o plano — e classifique:

| Tipo de erro do plano | Como detectar | Exemplo medido (15/08/2026) |
|---|---|---|
| **Código que ninguém rodou** | executor relata `TypeError`, atributo inexistente, import faltando | fixture com `__import__("app.main").app`; `erro(code, msg, msg=msg)` levantando `TypeError` |
| **Comando que não faz o que diz** | executor relata "não selecionou nada" / exit 5 | `pytest -k path_diff` com zero testes de nome correspondente |
| **Contagem inventada** | "esperado 6 PASS", vieram 8 | dois Steps seguidos com o número errado |
| **Lote declarado disjunto que não era** | conflito de merge | `git_ops.py` na Task 3 por desenho e na Task 1 por um Step |
| **Defeito que o plano carregou adiante** | achado numa Task tardia com origem numa antiga | `motivo` em português desde a Task 3, visto na 11, virou Task extra |
| **Barra pedindo o que o código reusado não faz** | divergência mock × componente existente | coluna de número de linha que o `DiffView` não tem |

**A causa comum dos seis é uma só: o plano descreve código que quem escreveu nunca executou.** Se
essa linha aparecer de novo, o patch não é mais uma régua de execução — é uma régua de
`planejamento.md`.

### 5. As fichas de modelo

`references/modelos/<provider>-<id>.md`, uma por modelo do time. Para cada um que trabalhou:

- **Números novos:** contexto por tipo de Task, tempo, custo. É o que faz o próximo plano prever
  rotação em vez de descobrir no meio.
- **Como ele falhou**, em padrão — e só vira afirmação com duas execuções concordando. Uma vez entra
  marcado `(visto uma vez, em <data>)`.
- **O que o kick-off teve que dizer por causa dele**, e que na próxima já pode nascer no plano.

Modelo sem ficha ganha a primeira. As regras do formato estão em `references/modelos/README.md`:
só coisa medida, com data, e cada linha responde *o que eu escrevo diferente por causa disto*.

### 6. Patch proposto

Arquivo, seção, e o texto pronto para colar. Com a evidência junto: *"medido em `<data>`: `<número>`"*.
Sem número, não entra — a skill é feita de coisa medida, não de impressão.

## O que NÃO entra

- **Elogio e resumo do que deu certo.** O que funcionou já está na skill; repetir gasta linha de
  quem vai ler.
- **Régua para caso que aconteceu uma vez** e tinha causa externa (cota estourada, máquina cheia).
  Isso vira nota no relatório, não patch.
- **Reescrita de critério.** Você propõe como o trabalho é conduzido, nunca o que conta como pronto.

## Onde salvar, e quem aplica

```
~/.claude/orq-retros/<data>-<gid>.md
```

O patch é **proposta**. Quem aplica na skill é **o usuário** — e essa trava é o ponto inteiro: uma
skill que se reescreve sozinha ao fim de cada execução acumula o viés de quem acabou de executar, que
é justamente quem não enxergou o problema enquanto ele acontecia.

Entregue ao árbitro: o caminho do arquivo e **as três linhas mais importantes**, não o relatório
inteiro. Ele repassa ao usuário.

## Como esta fase é lembrada sem ninguém lembrar

Três camadas, porque a única que funciona é a que **outro** dispara:

1. **No lançamento**, o árbitro escreve a retrospectiva no registro como item próprio, com gatilho —
   junto da revisão final, antes de abrir a primeira sessão. Nunca "no fim, de memória".
2. **O revisor final lembra.** O kick-off dele manda: ao entregar o `APROVA` da branch, dizer ao
   árbitro que **falta a fase 5**. Quem está fresco lembra; quem está no fim de um trabalho de doze
   Tasks, não.
3. **A definição de pronto**, na tabela de fases: o trabalho acaba quando o patch está na mão do
   usuário, não quando a branch é aprovada.
