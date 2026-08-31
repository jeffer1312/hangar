# Exceção: Tasks em paralelo, uma worktree cada

**O padrão é serial.** Um escritor por árvore, portão fechando cada Task antes da próxima
abrir. Não mude isso porque o plano é grande — mude porque as Tasks são **de verdade**
independentes e o trabalho é grande o bastante pra pagar a montagem.

Antes de pensar em worktree, lembre que o executor já paraleliza dentro de **uma** árvore:
um braço (subagente) por conjunto de arquivos disjunto, todos de uma vez, verificação uma
vez depois do join (`executor.md`, "Seus braços"). Isso entrega quase todo o ganho de tempo
com **zero** risco de merge, porque nunca existe uma segunda base. Worktree só se paga
quando a Task é grande o bastante pra justificar uma **sessão inteira** por ela.

## Gatilho — as quatro condições valem juntas

**Quem responde as quatro é a AUDITORIA do planejador, não o plano.** Nenhum método é obrigado a
entregar isto escrito, e vários não entregam — o planejador levanta ele mesmo, com o comando do item
3 do portão de saída (`planejamento.md`): arquivos por Task × `git merge-tree`, saída colada — do
texto dos passos quando o material os declara, **do repo, por subagente, quando não declara**. Esperar que o plano declare independência é o defeito, não a
prudência: a declaração de 15/08/2026 estava escrita, e era falsa. A auditoria é a fase 1, com o
usuário; o árbitro não deduz depois.

**Repo novo não dispensa a auditoria — muda onde ela olha.** Em código que já existe, o `grep` acha
o singleton retido. Em projeto do zero, o estado compartilhado ainda não está no disco: ele vai ser
**criado pelas Tasks do próprio lote** (o store da conversa, a conexão de socket, o cliente HTTP com
renovação de sessão). Então a condição 3 se audita no **desenho** — quem cria o quê e quem consome —,
não no repo. Nada aqui é freio a projeto novo: as quatro condições passam mais fácil num greenfield,
e é para elas serem respondidas rápido que existe o comando.

1. **Arquivos disjuntos.** Nenhum arquivo aparece em duas Tasks do lote. Não é "quase" nem
   "só o `types.ts`" — um arquivo compartilhado já é a serialização voltando pela porta dos
   fundos, com merge no meio.
   **Confira nos PASSOS, não no cabeçalho da Task.** Foi ali que a colisão de
   15/08/2026 se escondeu: o cabeçalho da Task 1 não citava o arquivo e o passo 8 dela mandava
   editá-lo — a declaração "nenhum arquivo em comum" estava escrita e era falsa.
2. **Nenhum símbolo atravessa.** Nada que a Task A cria é consumido pela Task B do mesmo
   lote. Se B espera uma função que A ainda está escrevendo, B trabalha contra o vazio.
3. **Nenhum ESTADO compartilhado.** Store, singleton de módulo, registry, cache, tabela: duas
   Tasks que montam hosts do mesmo estado não são independentes por mais disjuntos que os
   arquivos sejam, e a colisão **não aparece no merge** — aparece em rodadas de revisão, que é
   onde ela custa mais caro. Medido em 16/08/2026: um store singleton retido por três componentes
   nascidos em três Tasks tratadas como independentes; **8 das 11 rodadas** das duas últimas foram
   um host escrevendo no estado que o outro limpa, lê ou apaga.
4. **Verificação isolada.** O comando de verificação de cada Task roda sozinho, na worktree
   dela, sem depender do que as outras fizeram.

Falhou uma → aquela Task sai do lote e volta pra fila serial. Lote de duas ou três; acima
disso a integração vira o gargalo e você perdeu o ganho de qualquer jeito.

**Passar nas quatro é o gatilho, não o fim da conta.** O que decide é o gatilho **mais** o custo de
montagem deste repo (a seção "O custo real", abaixo): ambiente por worktree, portas, navegador
único, hooks compartilhados. Lote que passa nas quatro e é barato de montar é o caminho certo — a
página existe para ser usada, não para ser evitada.

## O que NÃO muda

- **Um escritor por árvore continua valendo** — agora existem N árvores, cada uma com um
  escritor. A regra nunca foi "um escritor por trabalho".
- **O portão por Task continua igual.** O revisor revisa um hash na ponta da branch daquela
  Task. Pra ele muda menos que no serial: a ponta dele não anda debaixo dele.
- **O árbitro continua read-only no código.** Merge limpo é mecânico e é dele. Conflito não
  é — ver abaixo.
- Intocáveis, stage por caminho explícito, sem `--amend`, receita de seis campos: tudo igual.

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
principal e o **revisor julga a correção antes do commit dela**, como em qualquer rodada — árvore
suja, objeto congelado, APROVA, e só então o commit.

**Enquanto houver conserto aberto na principal, você PARA de mergear.** Antes isso não importava,
porque o conserto virava commit em minutos e a janela era curta; agora ele fica com a árvore suja
até o APROVA, e um merge que entre nesse meio-tempo mistura as duas coisas: `git merge` de arquivos
não sobrepostos passa sem conflito, e a verificação completa que você roda depois dele estaria
rodando sobre código **não revisado** de outra Task — verde que não prova nada, vermelho cobrado do
executor errado. Um merge parado alguns minutos é mais barato que uma verificação sem significado. Você não vira quem
diz que o código está certo só porque a verificação ficou verde na tua mão — a única coisa que
fecha portão neste tubo é `APROVA` de quem revisa, e é justamente na integração, onde a
tentação de resolver sozinho é maior, que essa regra precisa estar escrita.

Terminou o lote: `git worktree remove` em cada uma. Worktree órfã é a próxima sessão
trabalhando num checkout que ninguém explica.

**E a conferência de rastro roda ANTES de remover**, procurando o caminho da worktree em toda
configuração global, não só nos symlinks: `grep -rl "<caminho da worktree>" ~/.local/bin <dirs de
configuração do agente> <dir de unidades de serviço>`. Removida a worktree, o rastro aponta pra um
caminho que não existe mais e o estrago passa a ser silencioso — medido em 18/08/2026: os hooks das
**três** contas do usuário apontavam pra uma worktree de prova, 10 ocorrências só na conta padrão.

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
tem que escrever. **A tabela de portas por Task vai no PLANO** (Task → `CP_PORT` → porta do vite)
— a execução de 16–17/08 escreveu e funcionou. Task visual em paralelo, na dúvida: serialize.

**E as portas não bastam: o NAVEGADOR de automação é um por máquina.** `agent-browser` e afins têm
uma aba só — dois executores capturando ao mesmo tempo roubam a página um do outro, sem erro
nenhum. Medido em 17/08/2026: a aba de uma executora devolveu a URL da Task vizinha às 13:44, e
ela passou 3h perguntando à página errada se a tela dela tinha voltado. Lote com 2+ Tasks
visuais: ou **instância de navegador própria por executor** (perfil/porta separados, se a máquina
suporta), ou **prova visual como seção crítica** — um executor captura por vez, o árbitro dá a
vez. O plano declara qual dos dois; o executor confere a aba antes de cada captura de qualquer
jeito (`executor.md`, passo 3).

**Arquivo ADITIVO compartilhado (catálogo i18n, índice de exports) não tira a Task do lote — mas o
plano diz COMO ele é tocado.** A página manda "arquivo compartilhado sai do lote", e isso não sobrevive
a um catálogo de tradução, que **toda** Task de tela toca. O conflito ali é **posicional por
construção** (as duas acrescentam no fim do arquivo) e o git não resolve nem quando um lado contém o
outro. Então: cada Task insere no bloco do **próprio prefixo**, em ordem alfabética — não no fim —, e o
gate do executor exige quebra de linha final (`tail -c1 | xxd` = `0a`). O conflito posicional que
sobrar **é do árbitro, no merge**: ele prova por **conteúdo** (contagem de chaves de cada lado antes e
depois, zero valor alterado) e resolve por estratégia de merge — **nunca devolve ao executor**, que não
tem como resolver isso na branch de origem. Medido em 22/08/2026: duas rodadas de mensagem gastas
tentando empurrar pro executor um conflito que só se resolvia no merge, uma delas por um único byte.

**Recurso GLOBAL do APARELHO é seção crítica como o navegador**: o aparelho em si, o encaminhamento
de porta (que é dele, não da sessão) e o armazenamento do app. Porta por Task no plano (T5→8083 …
T10→8086 funcionou) e encaminhamento refeito imediatamente antes de cada captura. Os executores
negociando o aparelho entre si por recado, com slots de 10–15 min, funcionou sem o árbitro no meio —
mas **sessão que morre segurando o recurso vira impasse silencioso**: medido em 22/08/2026, uma
revisora ficou parada mais de 30 minutos esperando um aparelho preso por uma sessão que tinha
morrido. Duas regras daí: **quem segura libera
ANTES de fechar o próprio trabalho**, e o árbitro **olha quem segura o quê** sempre que alguém fica
ocioso sem motivo aparente.

**Os hooks do git são compartilhados, e por isso worktree NÃO roda `git merge main`.** `.git/hooks`
vale para o checkout principal e para todas as worktrees. Um `post-merge` que rode o instalador do
projeto executa com o toplevel valendo **a worktree** — e reaponta unidades de serviço, symlinks
globais e o build do front para dentro dela. Medido em 17/08/2026, batendo no minuto nos dois
incidentes: o app do usuário saiu do ar duas vezes. **Quem integra a `main` é o árbitro, no checkout
principal**, onde o hook roda no lugar certo. Desligar hook do git é decisão do usuário, não saída do
time.

**Instalador NUNCA roda de dentro de worktree.** Symlink global apontando pra worktree morre com
ela: medido em 17/08/2026, remover uma worktree de ensaio levou 6 symlinks (`hangar-send`,
`hangar-engine`, 2 skills…), calou a vigia da máquina inteira e derrubou a statusline de toda sessão
Claude. Setup de máquina roda do checkout principal, sempre — e ao remover worktree, confira o
rastro: `ls -la ~/.local/bin | grep <worktree>`.

## Racionalizações — todas significam PARE

| Desculpa | Realidade |
|---|---|
| "O plano é grande, então paraleliza" | Tamanho não é independência. As quatro condições, ou serial. |
| "Os arquivos são disjuntos, então são independentes" | Estado compartilhado não é arquivo. Condição 3, 8 de 11 rodadas. |
| "Só o `types.ts` que as duas tocam" | Um arquivo compartilhado é merge no meio. Sai do lote. |
| "Resolvo esse conflitinho e sigo" | Você é read-only. Conflito é Task nova, serial. |
| "As duas passaram, mergeio as duas e verifico no fim" | Verificação depois de cada merge. Senão você não sabe qual quebrou. |
| "Já tem `APROVA`, não preciso reverificar depois do merge" | `APROVA` é "certo sozinho". A interação ninguém viu ainda. |
| "Deixo a worktree, depois eu limpo" | Worktree órfã é a próxima sessão no checkout errado. |
