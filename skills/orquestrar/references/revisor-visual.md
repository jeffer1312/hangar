# Revisor — o portão visual

Esta página é da **Task que mexe em pixel**. Se o diff não desenha nada, ela não é sua: volte para
`revisor.md` (procedimento) e `revisor-catalogo.md` (o que o parecer cobre).

## Sem prova de visão, é BLOQUEADOR

Task que muda o que aparece na tela só passa com evidência de que alguém **viu**: os
caminhos absolutos dos screenshots por estado, a pergunta visual feita a cada um, e o que
voltou. DOM, CSS e árvore de acessibilidade **não** substituem — eles provam que o elemento
existe, não que ele está legível, alinhado, ou que não virou um retângulo opaco sobre o
papel de parede.

O protocolo do executor sem visão está em `executor-visual.md`. Print anterior à correção não vale:
se ele consertou, tem que ter recapturado.

**Task com barra: o veredito cego vem junto, ou é BLOQUEADOR.** O plano nomeia, pra toda Task
que mexe em pixel, contra o que o resultado é comparado — uma tela que dá pra abrir, no mesmo
estado e na mesma largura. O reporte do executor tem que trazer, por rodada: quem venceu,
**qual letra era o trabalho dele**, o maior buraco apontado e o que ele consertou. Reporte que
diz só "comparei e ficou bom" é o mesmo "está bom?" com outra roupa — bloqueia.

**Você NÃO refaz o protocolo cego do executor.** Ele já rodou, com subagente fresco e teto de
rodadas; refazê-lo é pagar de novo a parte mais cara da sua janela por uma resposta que você já tem.
O que é seu é **uma passada**, no fim, sobre o print final e a barra, procurando as duas coisas que
a dele não pega:

- **Barra trocada no meio** — ele comparou com um estado diferente, outra largura, ou uma
  versão da tela de referência que já mudou. Comparação contra a barra errada é evidência
  falsa, não evidência fraca.
- **Ele venceu e mesmo assim está errado** — a barra é o piso, não o teto. Vencer a
  comparação cega não perdoa retângulo opaco sobre o papel de parede, texto cortado, nem
  estado que ninguém capturou.

Numa execução em que o revisor refez a comparação cega em seis rodadas, o resultado foram seis
divergências e **zero** bloqueadores; os 24 bloqueadores daquele trabalho vieram todos do código.

**Barra é "está fiel ao mock?"; defeito de tela é "está quebrado?".** É a pergunta que separa as
duas coisas antes de você escrever o achado, e cada uma tem um fim diferente:

- **Cumprido o teto de rodadas, a barra ENCERRA** — e encerrada quer dizer que ninguém a refaz, nem
  você. Divergência estética que sobrar vira `REGISTRADO`. Perdeu as duas rodadas e ele commitou
  mesmo assim (é o que `executor-visual.md` manda fazer, com o risco declarado): **não** é bloqueador
  automático — você julga o buraco que sobrou.
- **Defeito de tela não tem teto**: sobreposição, texto ilegível, aviso que não aparece, alvo de
  toque pequeno, largura errada, foco preso ou perdido pra fora do modal. Continuam bloqueador cheio
  até fechar, e **não gastam o teto da barra**, porque não são sobre fidelidade. Sem essa separação
  a Task estoura o teto com a tela quebrada, que é o oposto do que o teto existe pra evitar.

Numa Task de tela a barra foi encerrada na rodada 2 por decisão do árbitro e as rodadas 3 a 5 ainda
acharam **cinco bloqueadores**, nenhum de fidelidade; outra fechou em quatro rodadas, **só a
primeira de barra**.

**Rodada que não toca pixel não paga barra de novo.** Commit de correção que só mexe em store, teste
ou backend não refaz comparação nenhuma — o `git show --stat` prova, e a tua janela vai toda pro
código.

**Task que mexe em pixel e não tem barra nenhuma no contrato: `DEVOLVIDO`.** Não é bloqueador
de código — é decisão da fase 1 que ninguém tomou, e problema de processo não vira achado
técnico. Devolva ao árbitro dizendo *"Task N desenha tela e o contrato não traz barra nem
dispensa; a barra é decisão do usuário"*, e pare por aí: você não propõe a barra, não escolhe
uma, e não julga como se ela existisse. As duas coisas que a falta de barra faria em silêncio
— o executor pular a comparação cega e você aprovar sem cobrar — são exatamente o que este
`DEVOLVIDO` tira do silêncio.

**Contrato dizendo `Barra: nenhuma — decisão do usuário`: julgue normal.** A Task passa pelo
portão visual sem a comparação cega (prints por estado, você olha o conjunto no fim, estado
faltando continua sendo achado) e **você não cobra barra nenhuma**. Escolha registrada do
usuário é ordem, não lacuna — cobrar barra depois que ele dispensou é reabrir decisão já
tomada.

**Como olhar sem torrar contexto:** não acompanhe print por print enquanto o trabalho anda. Quem
captura descreve — o executor e a tua sessão verificadora têm como enxergar (comando de visão local
ou subagente de visão; numa máquina com o helper `see`, é ele). Deixe os dois trabalharem e, **no
fim, abra TODOS os prints de uma vez** e confira se cada um mostra o que você precisava. Uma passada
sua, no fim, sobre o conjunto — não uma leitura tua por imagem.

E **afirmação de símbolo se confere ampliada**: na passada final, sinal e cor citados na legenda
valem contra o recorte, não contra a imagem inteira — duas leituras a olho já chamaram de `✗` uma
pastilha `✓` verde.

O que essa passada final procura: print que não prova o que a legenda diz, estado capturado no
momento errado (antes da correção, com a tela em transição), e principalmente **estado que ninguém
capturou** — estado faltando é achado. Descrição de quem capturou é insumo; a conclusão é sua, e a
única forma de ela valer é você ter olhado o conjunto. Se **você** também não enxerga imagem e a
Task é visual, diga ao árbitro: revisor cego julgando tela é o portão não existindo.

