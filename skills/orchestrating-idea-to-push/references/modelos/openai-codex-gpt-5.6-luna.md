# `openai-codex/gpt-5.6-luna` — papel: REVISOR

Primeira ficha: 15/08/2026, trabalho de 13 Tasks. Cinco sessões revisoras, todas rotacionadas por
contexto.

## Números — e o teto aqui é o assunto principal

- **Janela 272k.** É o modelo de janela curta do time, e isso governa tudo o que vem abaixo.
- **Teto prático: 240k.** Entrar numa Task grande acima disso é compactar no meio do trabalho.
- **Custo por sessão de revisão:** baixo (US$ 0,05 a 0,15 por Task revisada).
- **Consumo por tipo de Task revisada**, medido:
  - Task de texto/teste: ~20 a 30k
  - Task de módulo (código + medição + leitura em volta): ~60 a 130k
  - **Task de tela: não medido, e ele mesmo recusou** — comparação cega gasta contexto de imagem de
    um jeito que ele não conseguiu estimar antes de ver quantos estados eram.

**Consequência para o plano: ele revisa 2 a 3 Tasks por sessão, e nenhuma de tela sem folga larga.**
Um trabalho de 12 Tasks precisa prever 4 ou 5 sessões dele. Preveja no plano em vez de descobrir.

## Enxerga imagem: sim, mas é o recurso caro dele

Comparação cega de print é a coisa que mais consome. Se o trabalho tem várias Tasks de tela, vale
pôr um revisor de janela larga nelas e deixar as de código para ele.

## Como ele falha

- **Receita que fecha o caso nomeado, não a família.** Três rodadas seguidas num trabalho: cada
  parecer fechava o caminho que o anterior citara, e a solução crescia. Ele **assumiu** o próprio
  furo nas duas vezes em que aconteceu, com medição junto — mas quem tem que cortar isso é o
  árbitro, pela linha de desperdício.
- **Enumera a lista incompleta na receita.** "Trocar nos usos X, Y, Z" quando eram seis — o executor
  fez os seis e avisou; se tivesse feito os três, o bug voltava.

## Onde ele é bom

- **Reproduz antes de afirmar**, e diz o comando. Pareceres dele vêm com "verifiquei por outro
  caminho" com frequência.
- **Assume erro próprio na cara**, incluindo "esta rodada extra é culpa da minha receita" — o que
  economiza uma investigação inteira do árbitro.
- **Recusa despachar ferramenta no alvo errado** e diz por quê, em vez de colar um "nada a apontar"
  sobre código que a ferramenta não leu.
- Avisa o teto **antes** de pegar a Task grande, quando o kick-off pede o `ctx` junto do parecer.

## O que o kick-off precisa dizer por causa dele

- Reportar o `ctx` junto de **cada** parecer, com o comando de leitura.
- Que o teto dele é 240k e que ele deve avisar antes de pegar Task grande, não no meio.
- Na receita: **inventário completo** dos usos, não exemplos.
