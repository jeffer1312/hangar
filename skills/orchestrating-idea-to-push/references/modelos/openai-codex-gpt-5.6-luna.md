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
  - **Task de tela: ~96k por rodada, mais ~85k de leitura inicial a cada troca de sessão.** Medido
    em 16/08/2026: 8 sessões dele pra 9 rodadas em duas Tasks. Duas fecharam **acima da própria
    janela** (310k e 309k de 272k), isto é, compactaram no meio do julgamento — e a segunda já não
    conseguia reportar o próprio `ctx`. **Numa Task de tela, conte um revisor por rodada, e abra a
    substituta ANTES de a correção chegar.**

**Consequência para o plano: ele revisa 2 a 3 Tasks por sessão, e nenhuma de tela sem folga larga.**
Um trabalho de 12 Tasks precisa prever 4 ou 5 sessões dele. Preveja no plano em vez de descobrir.

## Enxerga imagem: sim, mas é o recurso caro dele

Comparação cega de print é a coisa que mais consome. Se o trabalho tem várias Tasks de tela, vale
pôr um revisor de janela larga nelas e deixar as de código para ele.

## Como ele falha

- **A receita nomeia a ENTRADA, não o ponto que causa** — e por isso fecha o caso nomeado, não a
  família. Confirmado, não é mais "visto uma vez": 8 rodadas em duas Tasks (16/08/2026), cada
  parecer fechando o vizinho do caso anterior, e a solução crescendo. Não é falta de rigor: é o que
  sobra de janela depois de ler a tela. Ele **assume** o próprio furo, com medição junto — mas quem
  corta isso é o árbitro, pela linha de desperdício. Ver `revisor.md`, "O inventário do símbolo não
  fecha a classe sozinho".
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
