# Exceção: Tasks em paralelo, uma worktree cada

**O padrão é serial.** Um escritor por árvore, portão fechando cada Task antes da próxima
abrir. Não mude isso porque o plano é grande — mude porque as Tasks são **de verdade**
independentes e o trabalho é grande o bastante pra pagar a montagem.

Antes de pensar em worktree, lembre que o executor já paraleliza dentro de **uma** árvore:
um braço (subagente) por conjunto de arquivos disjunto, todos de uma vez, verificação uma
vez depois do join (`executor.md`, "Seus braços"). Isso entrega quase todo o ganho de tempo
com **zero** risco de merge, porque nunca existe uma segunda base. Worktree só se paga
quando a Task é grande o bastante pra justificar uma **sessão inteira** por ela.

## Gatilho — as três condições valem juntas

O planejador declara isso na fase 1, com o usuário. O árbitro não deduz depois.

1. **Arquivos disjuntos.** Nenhum arquivo aparece em duas Tasks do lote. Não é "quase" nem
   "só o `types.ts`" — um arquivo compartilhado já é a serialização voltando pela porta dos
   fundos, com merge no meio.
2. **Nenhum símbolo atravessa.** Nada que a Task A cria é consumido pela Task B do mesmo
   lote. Se B espera uma função que A ainda está escrevendo, B trabalha contra o vazio.
3. **Verificação isolada.** O comando de verificação de cada Task roda sozinho, na worktree
   dela, sem depender do que as outras fizeram.

Falhou uma → aquela Task sai do lote e volta pra fila serial. Lote de duas ou três; acima
disso a integração vira o gargalo e você perdeu o ganho de qualquer jeito.

## O que NÃO muda

- **Um escritor por árvore continua valendo** — agora existem N árvores, cada uma com um
  escritor. A regra nunca foi "um escritor por trabalho".
- **O portão por Task continua igual.** O revisor revisa um hash na ponta da branch daquela
  Task. Pra ele muda menos que no serial: a ponta dele não anda debaixo dele.
- **O árbitro continua read-only no código.** Merge limpo é mecânico e é dele. Conflito não
  é — ver abaixo.
- Intocáveis, stage por caminho explícito, sem `--amend`, receita de cinco campos: tudo igual.

## A receita

```bash
BASE=$(git rev-parse HEAD)          # a MESMA base pra todas — anote no contrato
git worktree add /caminho/wt-t2 -b <trab>-t2 "$BASE"
git worktree add /caminho/wt-t3 -b <trab>-t3 "$BASE"
```

Uma sessão de executor por worktree, criada pela receita de sempre (`arbitro.md`, "Abrir uma
sessão"). O kick-off de cada uma leva **o caminho da worktree dela** como repo, a branch
dela, e `HEAD esperado` = `$BASE`. Errar isso é uma sessão trabalhando na árvore da outra.

O contrato registra o lote: quais Tasks, qual `$BASE`, qual worktree e branch de cada uma, e
a ordem de merge.

## A integração é do árbitro, e é mecânica

Uma branch de cada vez, **só depois do `APROVA` daquela Task**:

```bash
git merge --no-ff <trab>-t2
# verificação completa do plano, aqui, agora
```

Duas regras fecham o desenho:

- **Conflito de merge = as Tasks não eram independentes.** O árbitro **para e não resolve** —
  resolver conflito é escrever código, e ele não escreve. A Task perdedora vira Task nova,
  serial, em cima da base já mergeada, com o executor dela. O conflito vira sinal em vez de
  trabalho escondido dentro do papel errado.
- **Verificação completa depois de CADA merge**, não só no fim. Vermelho volta pro executor
  daquela Task, **mesmo com o `APROVA` isolado dela no bolso**. Aprovado isolado quer dizer
  "certo sozinho", e é exatamente essa a lacuna que o paralelo abre.

Vermelho pós-merge **volta pro ciclo inteiro**, igualzinho ao conflito: executor conserta na
principal, e o **revisor julga o commit de correção** antes de você fechar. Você não vira quem
diz que o código está certo só porque a verificação ficou verde na tua mão — a única coisa que
fecha portão neste tubo é `APROVA` de quem revisa, e é justamente na integração, onde a
tentação de resolver sozinho é maior, que essa regra precisa estar escrita.

Terminou o lote: `git worktree remove` em cada uma. Worktree órfã é a próxima sessão
trabalhando num checkout que ninguém explica.

## O portão que o paralelo cria

No serial, cada Task é revisada já em cima do que a anterior fez — a interação entra no
portão de graça. Em paralelo isso some, e o único lugar que enxerga as Tasks conversando é o
fim. Então, **em lote paralelo, a revisão final da fase 4 deixa de ser boa prática e vira
obrigatória**, sobre `$BASE..ponta`, em sessão nova que não participou de nada. Registre no
contrato junto com o lote, não depois.

## O custo real, antes de você achar que é de graça

Neste repo cada worktree quer o **próprio** `node_modules` e `.venv`, e o portão visual
precisa de backend e Vite no ar — que brigam pelas portas 8765/5173. Duas Tasks visuais em
paralelo é uma delas esperando a porta, ou uma configuração de porta por worktree que alguém
tem que escrever. Task visual em paralelo, na dúvida: serialize.

## Racionalizações — todas significam PARE

| Desculpa | Realidade |
|---|---|
| "O plano é grande, então paraleliza" | Tamanho não é independência. As três condições, ou serial. |
| "Só o `types.ts` que as duas tocam" | Um arquivo compartilhado é merge no meio. Sai do lote. |
| "Resolvo esse conflitinho e sigo" | Você é read-only. Conflito é Task nova, serial. |
| "As duas passaram, mergeio as duas e verifico no fim" | Verificação depois de cada merge. Senão você não sabe qual quebrou. |
| "Já tem `APROVA`, não preciso reverificar depois do merge" | `APROVA` é "certo sozinho". A interação ninguém viu ainda. |
| "Deixo a worktree, depois eu limpo" | Worktree órfã é a próxima sessão no checkout errado. |
