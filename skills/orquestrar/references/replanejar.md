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
- **Task presa ANTES do portão.** Duas Tasks seguidas passando de 2× a estimativa **pela mesma
  causa** é um sinal — e é o sinal **tardio**, porque conta rodadas, e Task que trava antes do
  primeiro commit não produz rodada nenhuma para contar. O gatilho que teria disparado a tempo é
  mais simples: **Task com mais de 3 horas de relógio de executor e ZERO commits.** Medido em
  16–17/08/2026: duas Tasks somaram ~13h de relógio, 958k de saída e **nenhum commit** — logo,
  nenhuma rodada, nenhum veredito e nenhuma régua de espiral disparando. Depois do replanejamento,
  as **mesmas duas Tasks** fecharam em 2h52, com 329k de saída e dois merges.

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

## Replanejamento PREVISTO no plano — a miniatura deste procedimento

Plano pode declarar que a receita de uma Task só fecha no meio da execução ("a Task N depende do
que a medição da Task N-1 provar"). Isso é legítimo — e é **planejamento, não condução**: quem
fecha a receita é a sessão **planejadora** (ou uma sessão nova de planejamento, com a spec e o
documento da medição anexados), nunca o árbitro por gravidade. O árbitro entrega os insumos,
recebe a receita pronta e a recorta pro kick-off, como faria com qualquer Task. E a receita
fechada no meio passa pelo formato de Step da fase 1 (os três desfechos de request, o gatilho de
quem digita — `planejamento.md`), porque ela É plano.

Medido em 19–20/08/2026: o plano dizia "o árbitro fecha a receita da T5 depois da T4"; o árbitro
— sem o contexto do planejamento — fechou-a sem dizer QUANDO a chamada dispara, e dessa lacuna
nasceu o bloqueador mais sério do trabalho (a sonda que digita rodando no mount de toda conversa
aberta). A miniatura não exige o portão de 12 itens de novo: exige só o dono certo e o formato de
Step.

## O que o replanejamento NÃO é

- **Não é auditoria da execução** — isso é a retrospectiva (fase 5), que continua no fim.
- **Não reabre Task aprovada.** Defeito em Task mergeada é achado de revisão (de conjunto ou
  final), que vira Task nova no plano novo pelo ciclo normal.
- **Não é licença pra decidir time, conta ou branch** — as três continuam do usuário.
- **Não se repete como rotina.** Dois replanejamentos no mesmo trabalho = o problema não é o
  plano; pare e discuta o trabalho em si com o usuário.

## O procedimento pagou na estreia

O número é este: **13h sem merge viraram 2h52 com dois merges**, por ~2h15 de replanejamento (análise
pronta 18:31 → primeira Task em voo 19:20), medido em 17–18/08/2026. O que fez a diferença não foi o
plano ser novo — foi ele nascer com os artefatos que o portão da fase 1 cobra: estimativa a priori
(que segurou a fase em +23%), captura como Task própria com lista fechada, e a fumaça contra a fonte
real, que **destapou três bloqueadores na primeira vez que foi rodada**, depois de duas rodadas de
suíte verde não terem visto nenhum.
