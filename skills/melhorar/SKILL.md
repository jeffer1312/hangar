---
name: melhorar
description: |
  Use quando o usuario pedir para melhorar uma skill com base no que aconteceu de verdade - "melhora a skill X", "/melhorar X", "essa skill esta errando sempre na mesma coisa", "o que a skill X deveria ter aprendido este mes", "revisa a skill X com o que a gente corrigiu". Ela le o feedback humano acumulado (as memorias de correcao, o git da propria skill, o material que a skill declarar) e devolve uma PROPOSTA de patch, com evidencia medida, pra o usuario aprovar. NAO use para - escrever skill nova (skill-creator), corrigir um erro pontual da skill agora (edita direto), retrospectiva de um trabalho da skill `orquestrar` (isso e a fase 5 dela, que continua existindo).
---

# Melhorar uma skill com o que já aconteceu

Uma skill boa envelhece: o usuário corrige a mesma coisa três vezes, a correção fica na conversa, e
a skill continua ensinando o que ele já desmentiu. Esta skill lê essas correções e propõe a edição.

**Ela propõe. Nunca edita, nunca commita.** Essa é a trava inteira do desenho: skill que se
reescreve sozinha acumula o viés de quem acabou de trabalhar, e é justamente quem não enxergou o
problema enquanto ele acontecia. Quem decide é o usuário; quem aplica é uma sessão, pelas regras
normais do repositório daquela skill.

Ela é **invocada**, não agendada: `/melhorar <skill>`. O material chega em rajada — um trabalho
grande, uma semana de correções —, e rodar em cadência gastaria turno em semana quieta.

## 1. De onde vem o material

**A skill-alvo declara onde mora o material dela**, numa linha do próprio `SKILL.md`. É o que
permite esta skill servir a qualquer outra sem conhecer nenhuma: ela lê a linha, não a skill.

```markdown
Material desta skill: ~/.claude/orq-retros/*/  ·  docs/plans/*.md
```

Formato: uma linha começando com `Material desta skill:`, caminhos separados por `·`, glob e `~`
permitidos. Ela nomeia o **resíduo do uso** — o que sobrou de quem usou a skill —, nunca a
documentação dela.

**Sem a linha, valem as duas fontes padrão**, que existem para toda skill:

1. **As memórias de correção do usuário.** Arquivos com `type: feedback` em
   `~/.claude*/projects/<projeto>/memory/`. É o melhor material que existe: são as palavras dele,
   escritas no momento em que ele corrigiu, já com o **porquê** junto — capturado onde o trabalho
   acontece, sem passo extra pra ninguém.
   **Varra todos os diretórios de configuração**, não só o padrão: cada conta tem o seu, e o
   material se divide entre eles. Medido em 28/08/2026 nesta máquina: **370 memórias de correção em
   199 projetos**, espalhadas por sete diretórios — olhar só o `~/.claude` devolve uma fração e faz
   parecer que não há material.
2. **O git da própria skill.** Toda edição que o usuário fez à mão no `SKILL.md` é uma correção com
   a razão dela na mensagem de commit: `git log -p -- <diretório da skill>`. Sinal altíssimo e
   custo baixo — é a fonte que diz o que ele já teve de consertar sozinho.

**Os transcritos são a última fonte, e só com pedido.** Volume máximo, sinal pior, custo alto, e
eles carregam trabalho real do usuário. Nunca varredura ampla por iniciativa própria: se precisar,
peça, e busque dirigido — sessões que invocaram aquela skill e onde ele corrigiu logo depois.

## 2. Como ler: agrupe por CONDIÇÃO, nunca pelo nome do que quebrou

É aqui que uma melhoradora estraga a skill que devia melhorar. O material chega como incidente — uma
ferramenta que falhou, um arquivo que não existia, um dia em que deu errado — e o caminho fácil é
transcrever cada incidente como uma regra. O resultado é uma skill que sabe muito sobre o que já
aconteceu e nada sobre o que vai acontecer.

Leia duas vezes. Na primeira, junte os incidentes. Na segunda, pergunte de cada grupo: **qual é a
condição comum?** É ela que vira a régua; os incidentes viram a prova dela.

| O material diz | Não proponha | Proponha |
|---|---|---|
| "a ferramenta `<nome>` devolveu vazio porque o filtro dela não pega este tipo de arquivo" | uma regra sobre aquela ferramenta | "ferramenta com filtro próprio devolve 'nada a apontar' sobre código que não leu — confira se ela serve aos arquivos desta tarefa" |
| "o comando `<nome>` foi usado sem a metade que executa" | uma nota sobre aquele comando | "método invocado roda inteiro; metade ausente é bloqueio, não pendência" |

## 3. As quatro travas

**1. Régua se escreve como princípio; o caso medido entra como prova.** As duas coisas, sempre — o
princípio na frente **e** a medição junto. O teste, antes de propor qualquer uma: *onde deveria
estar a condição, você escreveu o nome de uma ferramenta, de um arquivo, de uma pessoa ou de uma
data?* Então é a instância. Reescreva a condição e mova o nome pra linha da prova.

**2. Piso de duas ocorrências.** Uma vez é azar — vira nota no relatório, não patch. Duas
ocorrências da mesma condição são **uma** régua com duas provas embaixo, nunca duas réguas: quando
o mesmo princípio aparece por dois caminhos diferentes, isso é o sinal mais forte de que você achou
o princípio certo, e separá-los desperdiça exatamente essa força.

**3. Sem número, não entra.** Data e medição junto, ou a proposta não sai. Impressão sobre como a
skill "parece" estar errando não é material — é opinião de quem leu, e ninguém precisa dela.

**4. Toda proposta traz o que ela APAGA ou substitui.** É a trava contra o inchaço, e é a que se
esquece. Melhoradora que só soma é uma máquina de crescer arquivo: cada rodada acrescenta uma régua,
nada sai, e em alguns meses a skill é longa demais pra ser lida — e skill que ninguém lê inteira não
governa nada. Ou a proposta nomeia a régua que morreu (deixou de valer, virou código, foi absorvida
pela nova), ou ela diz **por que nada saiu**. Sem uma das duas, não está pronta.

Régua que sai não some sem deixar rastro: ela vai pro relatório, com a data e o motivo. História
mora no relatório; o que ainda vale mora na skill.

## 4. O que ela produz

Um arquivo, e só ele:

```
~/.claude/melhorias/<skill>-<data>.md
```

Cada proposta traz, nesta ordem: **arquivo e seção** onde entra · **o texto pronto pra colar** ·
**a evidência** (data, número, de onde saiu) · **o que sai** por causa dela. E, no fim do relatório,
o que você viu e **não** propôs: incidente de uma vez só, caso com causa externa, coisa que já está
escrita. Isso vale tanto quanto as propostas — evita que a rodada seguinte reencontre o mesmo
material e conclua diferente.

Entregue ao usuário **o caminho e as três linhas mais importantes**, não o relatório inteiro.

Ele quer ver o diff em vez de ler prosa? Deixe a edição numa **branch local, não enviada**, no
repositório daquela skill — é o mais perto de um pedido de revisão que existe aqui. Não é o padrão:
faça só se ele pedir.

## 5. O que ela não faz

- **Não edita a skill.** Nem "a correção óbvia", nem "só a vírgula".
- **Não commita e não faz push.** Skills moram em repositórios diferentes, e alguns são públicos.
- **Não muda o que conta como pronto.** Propõe como o trabalho é conduzido, nunca o critério de
  aceite — quem julga não reescreve a régua pela qual é julgado.
- **Não varre transcrito** por iniciativa própria.
- **Não roda sozinha.** Sem agendamento, sem gatilho automático.
- **Não julga quem usou a skill.** O material é sobre o que a skill não dizia; não é sobre a sessão
  que errou nem sobre o usuário que corrigiu.

## 6. Ela roda sobre si mesma

Nada melhora a melhoradora, e é o furo conhecido deste desenho. O antídoto é ela ser curta o
bastante pra caber na cabeça de quem lê — e rodar sobre si mesma de vez em quando, pelas mesmas
regras, com as mesmas quatro travas. Inclusive a quarta: uma rodada sobre esta skill que só
acrescenta linha está errada por construção.

## Passo de adoção — alguém tem que escrever a linha

As skills existentes não têm a linha `Material desta skill:`. Sem ela, todas caem no padrão
(memórias + git), que já funciona — mas skill com resíduo próprio entrega muito mais.

Ao rodar sobre uma skill pela primeira vez: pergunte ao usuário se aquela skill deixa resíduo em
algum lugar, e **proponha a linha junto do resto do patch**, como qualquer outra proposta. Não a
escreva por conta própria: onde o material mora é conhecimento dele, não seu.
