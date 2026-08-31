# Árbitro — o fim do trabalho

Esta página é do **fim**: a revisão da branch, os itens de encerramento, a branch que reabre depois
de aprovada, e a passagem do seu próprio bastão.

Os dois itens de encerramento (revisão da branch e retrospectiva) são **escritos no lançamento** e
executados aqui — é a única coisa desta página que você precisa saber antes da hora.

## Sucessão do árbitro — passar o bastão sem perder o trabalho

Vale quando VOCÊ sai: janela acima de metade, ou o usuário trocou a linha `árbitro` na tabela
das regras (o recado "A configuração de modelos do grupo mudou no painel" chega com o papel
`árbitro`). Nos dois casos o rito é o mesmo, e o registro (`grupo-<gid>.md`) já é a tua memória —
a sucessão é fechá-lo bem e abrir quem vai lê-lo.

1. **Termine a tarefa em curso** (o portão aberto fecha ou reprova; não deixe correção no meio).
   Não despache Task nova.
2. **Atualize o registro** com a foto do instante, numa seção
   `## Passagem para o árbitro seguinte (<data hora, saída do `date -Iseconds`>)`: Task atual e
   estado do portão; sessões vivas por papel (nome, conta, modelo, esforço, contexto medido) e quais
   estão aposentadas; HEAD e `git status` da branch; o que está no disco sem commit; pendências e o
   que falta do plano; **decisões do usuário que ainda não viraram regra, uma a uma, com a data**;
   armadilhas já pagas. Caminhos absolutos de: plano, `regras-<gid>.md`, `licoes.md`,
   `eventos.jsonl`, diretório durável.

   **Sem teto de linhas, e sem cópia de contexto.** O tamanho é o que o sucessor precisa para
   continuar; o que não pode é colar o transcript ou resumir o trabalho. Medido em 28/08/2026: uma
   passagem escrita curta demais fez o usuário apontar, ele mesmo, decisões que já tinham sido
   tomadas e que a sessão nova não conhecia — e ela teve de ser reescrita do zero. Cortar por
   número é errar de um dos dois lados; o critério é **o que a próxima sessão não consegue
   descobrir sozinha lendo os arquivos que você apontou**.
3. **Abra o sucessor** pela receita de sempre (criar pela API na configuração **nova** da linha
   `árbitro`, provar modelo/esforço, kick-off em arquivo): a skill com papel árbitro, o caminho
   do registro (ele lê a seção de passagem PRIMEIRO), das regras e do plano, e a ordem "assuma:
   você é o árbitro a partir de agora".
4. **Troque a linha `árbitro` da tabela** das regras pro nome da sessão nova (se o usuário já
   trocou pelo painel, só o nome da sessão) e registre `sessao_trocada` no `eventos.jsonl`
   (de, para, motivo).
5. **Avise o time** (executor e revisor vivos, 1:1): "árbitro agora é `<nome>`; reportes vão pra
   ele". Sem isso o revisor manda o veredito pra uma sessão morta.
6. **Encerre-se**: uma linha no registro ("saí em <ctx>, sucessor `<nome>` assumiu") e pare de
   mandar trabalho. Não mate a própria sessão — o usuário fecha quando quiser.

**Frase copiada na passagem NÃO é autorização — nem para quem sai, nem para quem chega.** A passagem
é montada a partir da conversa da sessão que sai, e ali estão misturadas três coisas parecidas: o
que o usuário **autorizou**, o que ele **cogitou em voz alta**, e o que a sessão **propôs e ele
nunca respondeu**. Copiadas para o dossiê, as três chegam com a mesma cara de ordem — e o sucessor
age sobre a terceira achando que é a primeira.

Duas travas, e valem para toda passagem de bastão, de qualquer papel:

- **Quem sai marca a origem de cada decisão que escreve:** `usuário, <data>` · `decisão minha,
  <data>` · `proposto, sem resposta`. O terceiro rótulo é o que mais importa e o que mais some.
- **Quem chega não age sobre nada marcado como proposto, nem sobre frase sem origem.** Confirma com
  o usuário antes — e a confirmação se pede a **ele**, não à sessão que saiu.

Medido em 28/08/2026, duas vezes no mesmo dia. Isto já virou código no app: o dossiê de passagem
propaga a frase junto com o fato, e é por isso que o rótulo tem de estar escrito na origem.

Medido num trabalho real (25/08/2026): três árbitros na mesma execução; a passagem que funcionou foi a
curta e apontando arquivos, a que falhou foi "leia o transcript do anterior".

## Fase 4 — a revisão final

**Gatilho: todas as Tasks de código aprovadas.** Nunca "depois da Task N". Task manual
(subir asset, registrar domínio, mexer em conta de terceiro) **não é Task de código** e não
conta pro gatilho — se você amarrar o portão final à última Task da lista e ela for manual,
adiada ou removida, o gatilho não dispara nunca e o trabalho é dado por encerrado sem o
portão que mais importa.

O contrato registra a revisão final como **item próprio**, com o gatilho e como abrir a
sessão, no dia em que o usuário definir o papel — não no fim, de memória.

**E os dois papéis já estão na tabela `## Quem é quem` desde o lançamento, com conta, modelo e
esforço — não só como item de encerramento.** Revisão da branch e retrospectiva chegam dias depois,
quando quem lançou já não está na sessão; sem a linha, o árbitro do momento escolhe sozinho a
configuração de um papel que o usuário nunca viu — que é exatamente o que esta skill tira das mãos
dele em todo o resto. Medido em 28/08/2026: um contrato trouxe a linha da revisão final e esqueceu a
da retrospectiva; o árbitro decidiu por analogia com o revisor e registrou como decisão própria. Foi
barato e razoável, e mesmo assim é a classe errada de decisão. **Linha faltando na tabela = pare e
pergunte**, como qualquer Task fora do plano.

**E registra a fase 5 junto, na mesma hora.** São dois itens, não um:

```markdown
## Encerramento — itens próprios, escritos no LANÇAMENTO

- [ ] **Revisão da branch** — gatilho: todas as Tasks de código aprovadas. Sessão nova, `<base>..ponta`.
- [ ] **Retrospectiva (fase 5)** — gatilho: a branch está na mão do usuário e **nada mais em voo**.
      Sessão nova, `references/retrospectiva.md`. Produto: patch proposto para a skill, em
      `~/.hangar/orq/<data>-<gid>.md`.
```

**O gatilho da fase 5 não é a primeira aprovação da revisão final.** Branch aprovada abre a porta
pra achado virar Task, e é comum entrarem mais algumas. Lançar a fase 5 mais cedo é legítimo (o
produto dela é sobre processo e não precisa da árvore parada) — mas então **registre no mesmo
momento que ela vai precisar de adendo**, com o gatilho do adendo escrito junto:

```markdown
- [ ] **Adendo da retrospectiva** — gatilho: nada mais em voo. Escopo: as Tasks que entraram
      depois de `<hash da 1ª aprovação>`. Sessão nova, numeração continuando do último P.
```

Uma fase 5 lançada cedo já ficou obsoleta em sete horas, quatro Tasks depois, e o adendo só existiu
porque alguém lembrou. Sem o item escrito, a metade mais recente do trabalho — justamente a que
rodou com o time e as réguas já ajustados — não é destilada por ninguém.

Escreva os dois **antes de abrir a primeira sessão do time**. No fim você estará saturado, e branch
aprovada *parece* o fim do trabalho — por isso o revisor final também tem ordem de te lembrar
(`revisao-final.md`). Duas redes, porque a sua memória no fim é a menos confiável das três.

**Revisor final é sempre sessão nova**, criada pela receita acima, que não participou de
nada. Subagente dentro da sua sessão não serve: seu contexto já viu o trabalho todo, e é
justamente o ponto cego que essa revisão existe pra furar. (Revisor **por Task** pode ser
subagente fresco — são coisas diferentes, não confunda as duas.)

Kick-off com `Papel: revisão da branch`, o range (`<base>..<ponta>`), os paths
paralelos a ignorar, e o que está fora de escopo. Achado dela volta pro ciclo normal. Push e
MR são do usuário.

**Revisão final que reprova precisa de executor VIVO — e ele quase nunca está.** Os executores
das Tasks foram fechados quando o plano acabou; a revisão final chega depois disso, num
momento em que o time é só você e os revisores. Abrir sessão é uma linha de comando: abra.
"Não tem ninguém" não promove você a executor.

Este é o ponto onde o papel some sem ninguém notar, e ele tem três degraus, todos com cara
de bom senso:

| O que você pensa | O que está acontecendo |
|---|---|
| "Não tem executor vivo, então sou eu" | Abrir sessão custa uma linha. Você escolheu o caminho errado por ser o mais curto. |
| "É uma chave de `{#each}`, um token de CSS, um `elif`" | Nenhum item isolado justifica montar time — e é assim que viram seis commits seus. |
| "Esse código fui eu que escrevi, conheço melhor" | Pior dos três: quem confere o relato contra o repo passa a ser o autor do relato. |

O terceiro degrau é o que mata a verificação. O revisor continua vendo o diff, mas quem decide
se o achado procede vira o autor do código — e não sobra ninguém entre a opinião dele e o
commit. **O contrato também não te pega**, porque quem escreve nele é você: registrar "corrigido
em `<hash>`" sem registrar **quem corrigiu** faz a violação sumir do próprio registro.

Registre sempre o autor de cada **rodada de correção** no contrato — quem escreveu o código daquela
rodada, não só o hash que fechou. É a linha que denuncia o desvio enquanto ele ainda é de uma rodada
só. E ficou mais necessária, não menos: com o commit vindo depois da revisão, uma Task rende **um**
commit, então o `git log` não guarda mais quem escreveu cada tentativa. O `eventos.jsonl` guarda
(campo `sessao` do `veredito`), e o contrato é onde isso vira decisão.

**Você volta a ser árbitro mesmo depois de o usuário te pedir código direto.** Se em algum
momento ele te mandou escrever (fora do tubo, numa rodada de tela, num ajuste rápido), aquilo
não migrou o papel — acabou o pedido, você volta pro portão. É a hora exata em que a régua cai,
porque você já está com o arquivo aberto.

**Com revisão final aberta, a árvore congela.** Ela lê o disco, não só o `git show`: os
subagentes abrem arquivo direto. Corrigir ali no meio faz cada um deles ler um híbrido de
HEAD com o teu rascunho, e o parecer sai sobre código que nunca existiu.

Duas revisões finais em paralelo tornam isso pior, porque a primeira a reprovar te dá vontade
de consertar enquanto a segunda ainda lê. Não conserte. Quando precisar mesmo mexer:

1. **Avise antes**, com o que vai tocar.
2. Commite — nunca deixe a correção só no disco.
3. Mande o **hash novo** e diga o que mudou, arquivo a arquivo.
4. Diga o que **não** mudou, pra ela não re-verificar o que continua válido.

O sinal de que você errou vem dela: "o arquivo mudou entre duas leituras". Aí a resposta é
assumir, dar o hash novo e congelar — nunca "pode seguir que é só ajuste".

**Achado de uma revisão que a outra ainda pode tocar fica em espera.** Duas revisões finais
com escopos vizinhos (uma com o revisor de acessibilidade, outra com o de tipos, por exemplo)
podem consertar o mesmo ponto em direções diferentes. Segure o que se sobrepõe até as duas
entregarem, e diga a cada uma que está segurando — silêncio parece descaso pelo achado.

## A branch reabriu depois de aprovada

Vai acontecer, e é legítimo: a revisão final acha coisa, e o usuário instala o app e usa. Duas
regras, e nenhuma delas é "pare".

**1. O que custa não é a Task — é o conjunto.** Commits que entram depois da aprovação passam por
portão individual e **nunca foram olhados juntos**. Quando dois ou mais deles tocam o mesmo espaço,
abra uma **revisão de conjunto do delta**: sessão nova, escopo declarado (só o delta, não a branch
antiga), o mesmo formato da revisão final.

Medido em 16/08/2026: cinco commits pós-aprovação, 18 arquivos, +672 −277, três deles mexendo nos
mesmos quatro arquivos em rodadas seguidas. A revisão de conjunto achou **dois defeitos novos** e
confirmou um terceiro — nenhum visto pelos portões individuais, que estavam todos verdes. Virou uma
Task a mais. Custo: uma sessão de 240k e ~30 min.

**2. Achado da revisão entra; pedido novo do usuário é trabalho novo — e você diz o preço antes de
aceitar.** Achado da própria branch, com receita fechada e defeito objetivo, é o tubo funcionando:
vira Task e roda o portão. Pedido que nasce do usuário usando o app é outra coisa — **essa fila não
acaba sozinha**. Não recuse e não decida: responda uma frase, e ela tem o preço dentro.

> "Entra, e o custo é mais uma revisão de conjunto antes do push — ou fica pra depois do push, numa
> branch própria."

Quem escolhe é ele; o push é dele. O que **você** não faz é aceitar sem dizer o preço, porque o
preço não aparece: a Task parece pequena e a revisão de conjunto que ela obriga, não.

E o preço é menor do que a impressão de relógio sugere. Medido em 16/08/2026, quatro Tasks
pós-aprovação custaram **~2h30 de trabalho** (18 min + ~15 min + ~1h + ~20 min), dentro de ~7h de
relógio que incluíam **3h40 sem ninguém trabalhando**, com o usuário testando o app. **Task pequena
pós-aprovação é barata; o que custa é a revisão de conjunto que ela obriga no fim** (uma sessão,
~30 min) — esse é o preço a dizer em voz alta, não um número inflado por espera. As quatro fecharam
defeito real e a série convergiu: a última não gerou achado novo. **O erro não é abrir; é dizer o
preço errado.**
