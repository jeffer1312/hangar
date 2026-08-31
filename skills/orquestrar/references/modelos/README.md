# Fichas de modelo — o que cada um faz bem, mal, e o que isso muda no plano

Uma ficha por modelo, curta. Elas existem para o **planejador** ler antes de escrever o plano
(`planejamento.md`, "O TIME se decide ANTES de escrever o plano") e para a **retrospectiva**
atualizar no fim (`retrospectiva.md`).

**As fichas moram no cofre — `~/.hangar/orq/modelos/` — não nesta skill.** Elas são registro
datado da máquina de quem usa (a orquestração funciona inteira sem elas), e envelhecem por
construção. O que mora aqui é só esta página: a regra de como uma ficha se escreve.

## Regras destes arquivos

1. **Só coisa medida, com data.** "Parece melhor em X" não entra. "Em 15/08/2026, 3 rodadas contra
   2 das irmãs, 2,3× o custo do dia" entra.
2. **O que muda no PLANO, não elogio.** Cada linha da ficha responde: *o que eu escrevo diferente por
   causa disto?* Se não muda nada, não é ficha, é curiosidade.
3. **Curtas — 40 linhas.** Ficha que ninguém lê inteira não protege ninguém. O que envelheceu sai; a
   data denuncia.
4. **Não é tabela de contas.** Preço, cota e permissão vivem em `~/.claude/orquestracao-contas.md`,
   que é da máquina e quem decide é o usuário. Aqui é comportamento.
5. **Uma execução não faz ficha.** Padrão observado uma vez entra como "visto uma vez, em <data>".
   Duas execuções concordando viram afirmação.

## Nome do arquivo

`~/.hangar/orq/modelos/<provider>-<id>.md` — o mesmo par que o `--model` recebe, para não haver
dúvida de qual é.

## Modelo novo no time: pesquise antes, mas guarde separado

Modelo que nunca trabalhou aqui não tem ficha. Antes de escrever o plano, faça **uma** varredura
curta e registre numa seção própria. Duas fontes, e a segunda costuma valer mais:

- **O fabricante** — guia de prompting, notas da versão, limites publicados. Diz o que o modelo
  deveria fazer.
- **A comunidade** — o que quem usa de verdade descobriu, incluindo o que o fabricante não conta:
  onde ele quebra, que gambiarra virou padrão, com que ferramenta as pessoas contornam a limitação.
  Use a skill **`last30days`** para a varredura ampla (Reddit, HN, X, YouTube, últimos 30 dias) e
  depois vá fundo nas duas ou três fontes que aparecerem repetidas.

```markdown
## O que dizem  <!-- HIPÓTESE — não testado aqui -->
- <recomendação> — fabricante, lido em <data>
- <descoberta> — comunidade (<onde>), lido em <data>
```

Por que a comunidade paga mais: ela reporta a **limitação e o contorno juntos**. Dois exemplos que
chegaram por lá em 15/08/2026, e que nenhum guia oficial traria: que o modelo executor barato é cego
e existe um par de CLIs que dá visão a ele; e um loop de auto-melhoria em que um modelo executa e
outro anota o desperdício de cada volta — essa segunda virou régua desta skill no mesmo dia
(`revisor.md`, "linha de DESPERDÍCIO").

Duas regras, e a segunda é o ponto:

1. **Seção separada, sempre.** Guia do fabricante e medição nossa nunca se misturam no mesmo
   parágrafo — quem lê a ficha daqui a três meses precisa saber o que foi testado aqui e o que foi
   lido em algum lugar.
2. **Quando divergirem, ganha o medido, e a divergência fica escrita.** Ela é a informação mais
   valiosa da ficha inteira: é onde o modelo se comporta diferente do anunciado *neste* tipo de
   trabalho. Exemplo de 15/08/2026: o guia do Opus 5 desaconselha instrução extra de verificação
   ("ele verifica sozinho, e pedir mais faz verificar demais") — e o que resolveu aqui foi cobrar
   comando de leitura explícito, porque a falha não era falta de zelo, era truncar a própria
   conferência com `head`.

Recomendação do fabricante que **nunca foi testada aqui** fica marcada como hipótese até uma
execução confirmar. Não vire régua de kick-off sem medição: kick-off é onde o custo aparece.

## O que a ficha responde, na ordem

- **Janela e teto prático** — e quanto custa uma Task típica desse trabalho nele.
- **Enxerga imagem?** — decide se a barra visual precisa de código ou se print basta.
- **Como ele falha** — o padrão, não o caso isolado. É a seção que mais paga.
- **O que o kick-off precisa dizer por causa dele.**
- **Onde ele é bom** — para não desperdiçá-lo no papel errado.
