# Replanejar no meio: reescrever o plano e o contrato sem jogar fora o que já entrou

Não é papel fixo — é um procedimento, disparado no meio da fase 3, que roda a fase 1 **de novo,
menor, só sobre o que resta**. Nasceu de uma necessidade real (17/08/2026): um plano escrito num
método que o usuário abandonou, com duas Tasks presas e réguas novas que o plano não conhecia —
remendar Task a Task era jogar rodada fora, e não havia forma prevista de trocar o plano inteiro.

## Gatilhos — quem dispara é o usuário ou o árbitro propõe

- **O usuário mandou.** ("não confio nesse plano pra terminar")
- **Premissa central caiu**: uma decisão registrada do plano se provou falsa na execução, e mais
  de uma Task futura depende dela.
- **Método capenga**: o plano nasceu num método cuja metade executora não existe na máquina, ou o
  usuário decidiu trocar de método — troca de método **só** acontece por aqui (`SKILL.md`), nunca
  por emenda.
- **Estouro sistemático**: duas Tasks seguidas passando de 2× a estimativa **pela mesma causa** —
  é o sinal de que o desenho está errado, não a execução (mesma régua da espiral de rodadas).

O árbitro **propõe** ("replaneja, ou seguimos remendando? custo até agora: X"), o usuário decide.
Árbitro não replaneja por conta própria — e **não reescreve o próprio plano**: quem planejou de
novo é uma sessão com contexto limpo do viés de quem conduziu.

## Quem reescreve: um REPLANEJADOR — sessão fresca, com o usuário

Como a fase 1: o replanejador trabalha **com o usuário**, e o produto só vale com o "pode ir"
dele. A sessão é nova (ou o próprio usuário numa sessão de planejamento); o árbitro entrega os
insumos e **congela o grupo** enquanto isso (nenhuma Task nova abre; Task em voo termina ou é
suspensa com estado commitado).

O replanejador lê, nesta ordem:

1. **O que já está na base** — `git log` da branch: Tasks mergeadas são fatos, não opções.
2. **O que está em voo** — commit sem merge, worktree com diff sem commit: cada um vira decisão
   explícita no plano novo (aproveitar, revisar, descartar), nunca limbo.
3. **O contrato** (regras + registro) — as réguas que a execução fixou entram no plano novo como
   ponto de partida, não como descoberta a repetir.
4. **Os pareceres** — a linha de desperdício de cada rodada é o mapa do que o plano velho errou.
5. **O estimado × real** — onde havia; onde não havia, o custo medido por `git log`/transcripts.
6. O plano velho, por último — pra herdar o que ainda vale, não pra defender.

## O que o plano novo é

- **Cobre SÓ o trabalho restante.** Tasks mergeadas viram uma seção `## Base (fase anterior)` —
  fatos, com hash — e **não são renumeradas**: a barra de progresso e os pareceres antigos citam
  os números velhos.
- **Nasce no método do contrato** (padrão `superpowers`) — inteiro. Se o replanejamento é troca de
  método, o plano novo nasce 100% no método novo; formato misto é o defeito que a linha `Método:`
  existe pra impedir.
- **Arquivo novo** em `docs/superpowers/plans/`, com o nome do trabalho + `fase-final` (ou `v2`).
  O plano velho ganha, no topo, um aviso apontando pro novo — nunca é apagado: os pareceres o citam.
- **Passa o MESMO portão de saída da fase 1** (`planejamento.md`, checklist de 12 itens) —
  replanejamento não é atalho: estimativa a priori, não-colisão, barra/captura, fumaça, teto com
  cota, tudo de novo, agora com os números REAIS da fase anterior como base da estimativa.
- **O time volta a ser pergunta.** Como Task fora do plano (`arbitro.md`): o trabalho restante
  pode ser de outra natureza que o time original. Proponha com o histórico na mão; o usuário
  escolhe.
- **Branch volta a ser pergunta** (`planejamento.md`, fase 2) — inclusive "continuamos onde
  estamos", que é resposta legítima e registrada.

## O contrato acompanha — reescrito, não emendado

Aprovado o plano novo:

1. O árbitro (o da fase anterior, ou o replanejador assumindo — decisão do usuário, registrada)
   **reescreve `regras-<gid>.md` do zero** a partir do esqueleto da fase 2, apontando pro plano
   novo. Régua viva da fase anterior entra; régua de Task morta vira uma linha no registro.
2. O **registro continua o mesmo arquivo** (`grupo-<gid>.md`): uma entrada com data marca o
   replanejamento — motivo, o que morreu do plano velho, hash da base — e o diário segue.
3. **Todo kick-off a partir daí aponta pro plano e pras regras novas.** Sessão viva da fase
   anterior que for continuar recebe kick-off novo; a que não couber no time novo é aposentada
   com o de sempre (transcript lido, trabalho recuperado).

## O que o replanejamento NÃO é

- **Não é auditoria da execução** — isso é a retrospectiva (fase 5), que continua no fim.
- **Não reabre Task aprovada.** Defeito em Task mergeada é achado de revisão (de conjunto ou
  final), que vira Task nova no plano novo pelo ciclo normal.
- **Não é licença pra decidir time, conta ou branch** — as três continuam do usuário.
- **Não se repete como rotina.** Dois replanejamentos no mesmo trabalho = o problema não é o
  plano; pare e discuta o trabalho em si com o usuário.
