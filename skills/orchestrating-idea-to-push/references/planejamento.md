# Papel: planejador (fases 0, 1 e 2)

Você conduz o research, escreve a spec e o plano **com o usuário**, e lança o time. Quando o
plano é aprovado você vira o árbitro — e a partir dali não escreve mais código. Leia
`arbitro.md` nesse momento.

## Fase 0 — Research (só se o plano não sai sem ele)

Sessão ou subagente **read-only**, com a pergunta fechada ("como o fluxo X funciona hoje",
"o que quebra se mudar Y"). Saída é um arquivo em disco que o plano referencia — research
que só existe no contexto de uma sessão morre no `/clear`. Dá pra escrever o plano sem
isso? Pule.

## Fase 1 — Spec e plano

É aqui que as decisões se fecham; cada uma que ficar aberta é uma chance de acordar o
usuário às 3h da manhã. Além do que o `writing-plans` já pede, o plano carrega:

- **Ordem das Tasks** e quais não paralelizam, com o motivo.
- **Intocáveis**: paths com mudança paralela na árvore, listados um a um.
- **Verificação por Task**: o comando exato e o que conta como passou.
- **O que a revisão precisa cobrir** — ver abaixo. Isso entra **antes da Task 1**.
- **Decisões em aberto**: o que ainda não foi decidido e quem decide. Lista vazia é a meta.
- **Teto**: quanto de custo/cota o usuário aceita gastar sozinho, e o que faz parar.
- **O time**, com motor e conta de cada papel.

### O rigor da revisão entra no contrato antes da Task 1

Escreva no plano o que a revisão tem que quebrar: fluxo completo na UI ou no comando real,
callers irmãos do símbolo alterado, concorrência (resposta atrasada, duplo clique, troca de
alvo, unmount), estado final em disco/storage/URL, e quais skills de revisão usar por tipo
de Task.

Task visual entra com **a lista dos estados** que precisam de screenshot (as duas larguras,
overlay, tela cheia, o que mais a Task afetar). É essa lista que o revisor cobra depois —
estado que ninguém listou é estado que ninguém olha.

Sem isso as primeiras Tasks passam por um portão que ainda não existe, e o preço é uma
auditoria retroativa que reabre Task já aprovada — mais cara que ter escrito três linhas.

### O time é saída do planejamento

Quem escreve e quem revisa se decide **aqui**, porque o research e o brainstorming acabaram
de mostrar de que este trabalho é feito. Decidir no lançamento é decidir sem esse dado.

Não existe elenco padrão. Modelo citado em qualquer exemplo é exemplo — nunca default.
Olhe as Tasks e responda:

| Pergunta sobre o trabalho | O que ela decide |
|---|---|
| Cada Task é volume mecânico, raciocínio sutil ou julgamento visual? | quem escreve — pode ser **um escritor por Task** |
| O erro típico dela aparece em quê: teste, tela, carga, estado em disco? | o que o revisor precisa **conseguir fazer** (ver print, rodar harness, ler concorrência) |
| Tem Task visual? O executor escolhido enxerga imagem? | se não enxerga, o protocolo de visão do `executor.md` (`see`) é obrigatório e entra no contrato — não é motivo pra descartar o motor |
| Quem revisa pensa diferente de quem escreveu? | família do revisor — **não negociável** |
| Quanto custa e em qual conta? | os motores, e o teto |

Regras fixas:

- **Quem planejou não executa.** Vira árbitro, read-only no código pro resto do trabalho.
- **Revisor de família diferente do executor.**
- **Revisão final** em sessão nova que não participou de nada.
- Um escritor por árvore vale mesmo com vários escritores no elenco: o portão serializa as
  Tasks, então eles nunca escrevem ao mesmo tempo.

O plano registra a tabela com **cinco** colunas — papel, sessão, agente/motor, **qual conta
gasta**, e **como a sessão é aberta** (o comando literal). Motor de provedor consome a conta
dele: o usuário aprova isso aqui.

A coluna "como abrir" existe porque *"a revisão final é numa sessão do <agente> X"* é uma
frase que envelhece mal: meses depois, na hora de abrir, vira decisão improvisada entre
conta padrão, motor, gateway e subagente — e as quatro dão resultados diferentes. Escreva o
comando no dia em que o usuário definir o papel:

```markdown
| Papel | Sessão | Agente/motor | Conta | Como abrir |
|---|---|---|---|---|
| revisão final | <trab>-final | <agente>, conta padrão | assinatura | `cp-send --new <trab>-final <cwd>` (SEM --engine) |
```

**A revisão final entra na tabela como item próprio, com o gatilho junto:** *"dispara quando
todas as Tasks de código estiverem aprovadas"*. Nunca "depois da Task N" — Task manual
(subir asset, registrar domínio, mexer em conta) não é Task de código, e amarrar o portão
final a ela faz o gatilho não disparar nunca. A receita de abertura está em `arbitro.md`.

### Antes de aprovar

Passe o plano por um olhar adversarial (subagente de arquitetura + explorador): cada
arquivo/símbolo citado existe? A ordem se sustenta? O que quebra? Plano que cita símbolo
inexistente vira round perdido na execução.

## Fase 2 — Lançamento (o único "pode ir" do usuário)

### Pré-voo, antes de criar sessão

```bash
git status --short          # árvore suja → os paths viram intocáveis, listados um a um
git branch --show-current   # branch certa
cp-send --list              # QUEM mais está vivo neste cwd
```

Outra sessão escrevendo neste checkout trava a largada — resolva com ela, não com o usuário.
Não resolveu → aí sim é decisão dele.

### Criar, na ordem

```bash
cp-send --new <trab>-writer /caminho/do/repo --engine <motor do plano>
cp-send --new <trab>-review /caminho/do/repo --engine <outro motor>
cp-send --pair <sessao> "<trab>: <papel dela>"     # uma chamada por sessão
```

Ordem obrigatória: `--new` → `--pair` → ler o `gid` no próprio sidecar → **escrever o
contrato** → só então os kick-offs. Endereço apontando pra arquivo que ainda não existe é
uma sessão parada perguntando.

Motor inexistente devolve `400` e a sessão não nasce. Ver os motores: `claude-engine`.

### A sessão nova prova modelo e effort ao vivo

`cp-send --new --engine` **não** configura effort, e pedir "max" no primeiro prompt não
funciona. Antes de liberar a primeira Task, exija da sessão nova a prova ao vivo (o que a
statusline dela mostra, ou o retorno do comando de troca) — repetir o que o kick-off pediu
não é prova. Sem isso ela trabalha horas no effort errado afirmando que está no certo.

### Recado: nativo ou cp-send

Sessão no `ListAgents` e você tem `SendMessage` → `SendMessage`. Senão `cp-send <sessao>`.
`--new`, `--pair` e `--group` são sempre `cp-send`. Mensagem longa vai por heredoc de aspas
simples.
