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

## As cinco entradas

```bash
# 1. o registro do árbitro — o diário: Task → hash → veredito, rodadas, decisões com data
#    (mora no diretório durável; o `grupo-<gid>.md` do backend é apagado junto com o grupo)
cat ~/.hangar/orq/<data>-<gid>/registro.md

# 1b. os pareceres — a linha de DESPERDÍCIO de cada rodada é a matéria-prima da análise
ls ~/.hangar/orq/<data>-<gid>/pareceres/*.md

# 1c. os kick-offs — como cada sessão foi despachada (o que ela sabia ao começar)
ls ~/.hangar/orq/<data>-<gid>/kickoffs/

# 2. as LIÇÕES — toda régua que o árbitro precisou escrever no meio do trabalho
cat ~/.hangar/orq/<data>-<gid>/licoes.md

# 3. a branch: quantos commits por Task, quantas rodadas de correção
git log --oneline <base>..<ponta>

# 4. o que a PRÓPRIA SKILL ganhou durante a execução — é a evidência mais forte que existe
git -C <repo-da-skill> log --oneline --since="<data-de-início>" -- skills/orquestrar
git -C <repo-da-skill> diff <commit-antes-do-trabalho>..HEAD -- skills/orquestrar

# 5. o eventos.jsonl — rodadas, vereditos e tempos por task JÁ contados pelo árbitro
cat ~/.hangar/orq/<data>-<gid>/eventos.jsonl
```

A quinta é a que dá número sem recontar na mão: rodadas, vereditos e tempo por task já vêm
contados de lá, e a retro **confere a prosa contra ele** em vez de reconstruir de git e mtime.
Execução antiga não tem o arquivo — aí valem as quatro de sempre, e isso se diz no relatório.

A segunda e a quarta são as que ninguém pensa em olhar, e são as que mais entregam: **toda régua que
o árbitro precisou escrever no meio do trabalho é uma coisa que a skill não tinha.** Se ele teve que
decidir, escrever e avisar as sessões, a decisão não estava aqui. O `licoes.md` é essa lista já
pronta, com data e prova ao lado de cada uma — é a entrada mais barata que você tem.

## O que o relatório tem — cinco seções, nesta ordem

**Não existe seção de análise de tempo, e é decisão do usuário (28/08/2026).** Bloco a bloco,
estimado contra real, não muda o que a próxima execução faz: relógio de calendário é feito de espera
por decisão dele, de banco fora do ar e de VPN caindo, e separar isso do trabalho custa mais do que
rende. O que **realmente** custa execução são rodadas repetidas, e isso é a seção 1 abaixo. Task
estourando o relógio já é tratada quando acontece, pelo árbitro, e não em retrospecto. Se algum
número de tempo entrar aqui, ele vem de `date -Iseconds` ou do carimbo do git — nunca de memória:
medido neste mesmo trabalho, as horas escritas de cabeça no registro tinham desvio de até **+6h13**.

### 1. Desperdício agrupado

Junte as linhas de desperdício de **todos** os pareceres (`revisor.md`, "Formato do parecer") e
procure repetição. Uma vez é azar. **Três vezes é buraco da skill**, e o texto da terceira já é
quase a régua nova.

### 2. Réguas que nasceram no meio

Do `licoes.md` do grupo e do `git diff` da skill. Para cada uma:

| A régua | O que a fez nascer | Já está na skill? |
|---|---|---|

Régua que ficou só no arquivo do grupo **morre com o trabalho** — o grupo seguinte não a herda. É
exatamente essa a lista que vira patch.

### 3. O que o PLANO errou — e esta é a seção que mais paga

As outras duas olham como o trabalho foi conduzido. Esta olha o que foi **planejado**, e é onde está
o ganho maior: defeito de plano custa rodadas de execução, e custa em todas as Tasks que dependiam
dele.

Varra os relatos de "desvio declarado" dos executores no registro — cada um é um lugar onde a
realidade não bateu com o plano — e classifique:

| Tipo de erro do plano | Como detectar | Exemplo medido (15/08/2026) |
|---|---|---|
| **Código que ninguém rodou** | executor relata `TypeError`, atributo inexistente, import faltando | fixture com `__import__("app.main").app`; `erro(code, msg, msg=msg)` levantando `TypeError` |
| **Comando que não faz o que diz** | executor relata "não selecionou nada" / código de saída de "nada a rodar" | filtro de teste por nome que não casa com teste nenhum |
| **Contagem inventada** | "esperado 6 PASS", vieram 8 | dois passos seguidos com o número errado |
| **Lote declarado disjunto que não era** | conflito de merge | um arquivo na Task 3 por desenho e na Task 1 por um passo |
| **Defeito que o plano carregou adiante** | achado numa Task tardia com origem numa antiga | `motivo` em português desde a Task 3, visto na 11, virou Task extra |
| **Barra pedindo o que o código reusado não faz** | divergência mock × componente existente | coluna de número de linha que o `DiffView` não tem |

**A causa comum dos seis é uma só: o plano descreve código que quem escreveu nunca executou.** Se
essa linha aparecer de novo, o patch não é mais uma régua de execução — é uma régua de
`planejamento.md`.

### 4. As fichas de modelo

`references/modelos/<provider>-<id>.md`, uma por modelo do time. Para cada um que trabalhou:

- **Números novos:** contexto por tipo de Task, tempo, custo. É o que faz o próximo plano prever
  rotação em vez de descobrir no meio.
- **Como ele falhou**, em padrão — e só vira afirmação com duas execuções concordando. Uma vez entra
  marcado `(visto uma vez, em <data>)`.
- **O que o kick-off teve que dizer por causa dele**, e que na próxima já pode nascer no plano.

Modelo sem ficha ganha a primeira. As regras do formato estão em `references/modelos/README.md`:
só coisa medida, com data, e cada linha responde *o que eu escrevo diferente por causa disto*.

### 5. A proposta de mudança na skill (o "patch")

Cada proposta traz **quatro** campos, nesta ordem: **arquivo e seção** onde entra · **o texto pronto
pra colar** · **a evidência** (*"medido em `<data>`: `<número>`"*) · **o que SAI da skill por causa
dela**. Sem número, não entra — a skill é feita de coisa medida, não de impressão.

**O quarto campo é o que impede a skill de só inchar, e é o que se esquece.** Ou a proposta nomeia a
régua que morreu — deixou de valer, virou código, foi absorvida pela nova, era um número arbitrário
que virou princípio — ou ela diz, numa linha, **por que nada saiu**. Sem uma das duas, ela não está
pronta. Medido em 28/08/2026: um relatório com 18 propostas não trazia esse campo em nenhuma delas;
ao levantá-lo depois, quatro apagavam coisa, três já estavam escritas na skill e não precisavam ser
propostas, e cinco caíam todas na mesma seção de um arquivo, que dobraria de tamanho. Nada disso
era visível sem o campo.

Régua que sai não some sem rastro: ela vai pro relatório, com a data e o motivo. História mora no
relatório; o que ainda vale mora na skill.

E diga, no fim, **onde as propostas se concentram** — quantas caem em cada arquivo, e se alguma
seção recebe três ou mais. Seção que recebe muitas de uma vez não deve ser engordada: ou as novas
viram uma lista curta de conferência no fim dela, ou a seção vira arquivo próprio. Quem lê a skill
lê o arquivo inteiro; seção de cem linhas não é lida, é folheada.

**E a régua se enuncia como PRINCÍPIO; o caso medido entra como PROVA dele.** As duas exigências
valem juntas, não uma no lugar da outra: sem número a régua não entra, e escrita como caso ela não
serve. É a trava geral do `SKILL.md` ("Régua se escreve como PRINCÍPIO"), e é aqui que ela mais
pesa — a fase 5 é quem fabrica as réguas das próximas execuções, e você chega no fim de um trabalho
com os casos daquele trabalho na mão.

O teste, antes de propor qualquer régua: **onde deveria estar a CONDIÇÃO, você escreveu o nome de
uma skill, de uma ferramenta, de um arquivo ou de uma data?** Então é a instância. Reescreva a
condição e mova o nome pra linha da prova.

| Escrito como caso (não entra assim) | Escrito como princípio (entra) |
|---|---|
| "a skill `<nome>` roda capada quando invocada dentro de uma Task" | "skill invocada dentro de uma Task roda inteira — passo pulado é bloqueio, não pendência" |
| "o método `<nome>` foi usado sem a metade executora" | *(o mesmo princípio acima; o método é a segunda prova dele, não uma régua nova)* |
| "o revisor por linguagem `<nome>` não lê `.svelte`" | "ferramenta com filtro de extensão devolve 'nada a apontar' sobre código que não leu — confira se ela serve aos ARQUIVOS desta Task" |

**Duas provas do mesmo princípio não são duas réguas.** Se o registro trouxer dois incidentes que
caem na mesma condição, eles viram **uma** entrada do patch, com as duas medições embaixo — é o sinal
mais forte que existe de que você achou o princípio certo, e escrevê-los separados desperdiça
exatamente essa força. Ao varrer, agrupe por **condição**, nunca pelo nome da coisa que quebrou.

## O que NÃO entra

- **Elogio e resumo do que deu certo.** O que funcionou já está na skill; repetir gasta linha de
  quem vai ler.
- **Régua para caso que aconteceu uma vez** e tinha causa externa (cota estourada, máquina cheia).
  Isso vira nota no relatório, não patch.
- **Régua escrita como caso** — que nomeia a skill, a ferramenta, o arquivo ou a data no lugar da
  condição. Não é motivo pra descartar o achado: é motivo pra reescrever (seção 5).
- **Reescrita de critério.** Você propõe como o trabalho é conduzido, nunca o que conta como pronto.

## Onde salvar, e quem aplica

```
~/.hangar/orq/<data>-<gid>.md
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
