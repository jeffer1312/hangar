# `opencode-go/deepseek-v4-flash` — papel: EXECUTOR

Primeira ficha: 15/08/2026, trabalho de 13 Tasks (backend Python + front Svelte), 9 sessões
executoras. Uma execução — o que estiver marcado *(visto uma vez)* ainda não é padrão confirmado.

## Números

- **Janela 1M**, teto de trabalho 500k.
- **Task de backend/teste:** ~30 a 50 min, contexto ~150k.
- **Task de tela** (monta, abre navegador, despacha comparação): **~170k de contexto** e 1h+.
  Duas não cabem na mesma sessão com folga.
- Custo: a referência de assinatura barata do time. O `deepseek-v4-pro` foi testado no mesmo dia,
  em duas Tasks, com **o mesmo número de rodadas** e 2,3× o gasto — não pagou.

## Enxerga imagem: NÃO

Precisa de `see <caminho absoluto>` / `ver-front <url>`. Consequência para o plano: **a barra das
Tasks de peça solta tem que ser código** (o HTML/CSS do mock), não print. Onde a barra é print
mesmo (tela montada), o kick-off diz qual comando usar.

## Como ele falha

- **Decide por argumento quando o critério não é numérico.** O caso: mandado comparar densidade com
  o mock, argumentou que o app real mandava e commitou — errado em **sete** elementos, porque uma
  regra global de CSS estava comendo o componente. Quando o critério vira número
  (`getBoundingClientRect`), ele mede certo e acerta. **Régua visual no plano tem que ser número.**
- **Estima quando não tem o dado à mão.** Reportou "~150k estimado" de contexto; o real era 318k.
  Corrigiu sozinho depois. **Peça o comando, não o número:** "leia com `tmux capture-pane … | grep`".
- **Vai além do escopo quando o escopo não está fechado na cara.** Montou a tela da Task seguinte
  dentro da Task atual. Três dos quatro arquivos "a mais" que ele tocou estavam certos (descoberta
  real do hospedeiro), um não. **O kick-off precisa dizer o que NÃO é desta Task**, não só o que é.
- **Fecha o caso nomeado, não a família** — como todo executor sob receita. Quem tem que atacar a
  família é o parecer.
- **Aplica a receita ao pé da letra quando ela não contradiz o que ele mediu** (confirmado 8× em
  16/08/2026). Isso não é defeito dele: **transfere o gargalo pra qualidade da receita** — com uma
  receita que aponta a causa, ele fecha em 1 rodada com o menor diff da Task. **Quando contradiz,
  ele para e traz a medição:** desviou de um cleanup receitado, provou o desvio e o revisor lhe deu
  razão (1× em 16/08/2026). Ele não é cego à receita errada; só não a procura.

## O que o kick-off precisa dizer por causa dele

- Régua visual **em número**, com o comando de medição.
- O que **não** é desta Task (a tela da Task seguinte, o arquivo do lote vizinho).
- "Leia o valor com `<comando>`" em vez de "reporte o contexto".
- Que ele não enxerga imagem, e qual é o caminho (`see`).

## Onde ele é bom

**Aplica receita literal muito bem.** Reproduz a causa antes de editar quando mandado, mede depois,
e relata desvio da receita em vez de improvisar em silêncio — inclusive desvios que o revisor não
tinha visto (mediu que uma flag do `git` era global e não de subcomando, e disse). Vale investir no
detalhe do Step: com passo a passo bom, a taxa de acerto é alta.

Achou sozinho um erro de tipo do plano que ninguém tinha visto (o `changed` do git devolve
`R`/`C`/`U`/`T` além de `M`/`A`/`D`/`?`) *(visto uma vez)*.

Mais três coisas medidas em 16/08/2026:

- **Aguenta um bloco inteiro numa sessão só:** três Tasks de tela (11 rodadas somadas) fechando em
  **440k/1M**. O antecessor queimou 548k em 2 rodadas por ter chegado carregado — o kick-off que
  manda **esperar sem abrir código** antes do commit é o que faz a diferença.
- **Reproduz o defeito ANTES de editar quando o kick-off manda**, e a reprodução vale como registro
  do "antes" (dois visores medidos com coordenadas, `inert` ligado, prints numerados). Poupou o
  revisor de refazer a prova do estado anterior.
- **Acha defeito de CSS sutil e declara o risco por escrito:** mediu que `height: 100%` não
  funcionava porque o pai só tinha `max-height` (3676px antes, 652px depois), corrigiu com `calc`
  dos tokens e deixou comentado que o `calc` precisa acompanhar o padding. Task de conserto
  multi-arquivo (12 arquivos, +303 −199) em **uma rodada**, fechando em 92k.

## Medido em 17–18/08/2026 (5 sessões executoras da fase final + 3 da rodada pós-encerramento)

Números da fase final: 686 chamadas, 617.926 de saída, 129,8M de cacheRead, cartão **$0** (assinatura).

- **Contexto por Task, com a Task recortada e a captura fora:** 241k/1M numa Task de backend+tela
  (24% — longe do portão de 50%). Nenhuma rotação necessária em 8 sessões. O que segurou o número
  foi o recorte (3,7–5,2 KB por Task) e a captura ser Task de outra sessão.
- **Chama `request_compaction` sozinho, em "marco lógico"** — 3× nesta execução (241k e 187k de
  contexto jogados fora), uma delas **no meio da Task**, com o Step 4 inacabado, enquanto esperava
  resposta do árbitro. Proibido por escrito num kick-off → **zero** nas três sessões seguintes.
- **Reproduz o defeito antes de editar, e reproduz a sonda do revisor antes de consertar.** Segunda
  e terceira execuções concordando: isto passa de *(visto uma vez)* para **padrão**. Na rodada 2 de
  uma Task de poda ele rodou as duas sondas da revisora **antes** de tocar o código e mediu o depois
  (8/8 apagados → 0 apagados).
- **Lê o parecer inteiro em vez de aplicar por reflexo.** Três ocorrências: deixou de mexer num
  ponto vizinho citando a conferência da própria revisora de que ali não havia defeito; não foi por
  um caminho que ela já tinha testado e descartado, citando isso; e recolheu, sem ser cobrado, um
  item que ela havia registrado sem bloquear.
- **Corrige o árbitro com número.** Refez uma medição de sidecars órfãos que estava ~4× errada,
  porque o árbitro havia contado um diretório de configuração só.
- **Contesta acusação com fato** (`journalctl` + o próprio transcript) e **assume falta antes de ser
  perguntado** (narrou o próprio `pkill -f`, proibido pela régua, e passou a matar por PID exato).
- **Sobe palco sem isolar `HOME`** se o kick-off não exigir: uma prova ao vivo reescreveu o arquivo
  de configuração compartilhado das três contas do usuário e o deixou com JSON inválido. O kick-off
  precisa exigir `HOME` próprio, por escrito (`executor.md`, "O palco de prova não escreve fora da
  sua árvore").
