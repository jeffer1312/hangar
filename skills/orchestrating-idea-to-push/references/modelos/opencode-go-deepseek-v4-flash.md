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
