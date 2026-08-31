# Revisor — o catálogo do parecer

Esta página é do **momento em que você já tem o diff na mão** e está decidindo o que procurar. Ela
não é lista de tarefas: é o conjunto de perguntas cuja ausência já deixou defeito passar por um
portão. O procedimento — o que ler, para onde vai o parecer, o formato e a receita — está em
`revisor.md`, e a Task de tela em `revisor-visual.md`.

## O que o parecer precisa cobrir

Gate de tipos, build e testes passando é o **piso**, não o parecer. Além disso:

- **fluxo completo**, na UI ou no comando real, não só a unidade tocada;
- **callers irmãos**: quem mais usa o símbolo alterado tem a mesma causa?
- **concorrência**: resposta atrasada, duplo clique, troca de alvo no meio, unmount;
- **estado final**: o que ficou no disco/storage/URL depois — não só o retorno;
- **Task de orquestração (tmux, CLI, processo, conta): RODE o fumaça contra a fonte real, você
  mesmo.** Suíte verde de fakes não é prova de fluxo: um módulo já chegou com mais de três mil
  testes verdes e o fluxo morto — 405 linhas de teste novo provavam a suposição errada do próprio
  código, e quem pegou os 10 bloqueadores foi a revisora reproduzindo contra o tmux real. E
  **confira a CONTAGEM da suíte contra a base**: contagem que caiu sem nota no reporte é bloqueador
  por si (na mesma Task, uma unidade a menos, calada, escondia 7 testes de uma Task aprovada
  apagados). **E teste que troca a biblioteca inteira por um duplo prova que o botão chama a
  função, nunca para onde a função vai** — ver `executor-fluxo.md`, as duas metades da régua e as
  duas de desfecho (o conteúdo dos dois lados; a evidência trazendo o que distingue os dois
  caminhos).
- **o caso vazio**: código que **apaga**, que casa por semelhança, ou que decide a partir de uma
  lista de vivos — o que ele faz quando o conjunto vem **vazio**? Uma poda em que "não sei quem está
  vivo" virava "ninguém está vivo" já apagou 8 de 8 arquivos de sessão viva, fila incluída: a função
  que consulta devolve vazio sem levantar, então o `except` do autor nunca disparava. Régua curta:
  **lista de vivos vazia é motivo para NÃO apagar.**
- **a mesma regra escrita duas vezes**: dois lados que precisam concordar (backend e front, dois
  componentes, duas cópias do mesmo cliente) concordam **hoje** e nada garante amanhã. Já aconteceu
  com um piso duplicado nos dois lados: um deles ganhou uma noção nova, o outro ficou só com o piso,
  as regras divergiram e ninguém foi avisado.

**Branch cuja base não é a `main` atual: aritmética de suíte mente.** Compare **nomes** — inventário
dos nomes de teste do pai contra o commit; nenhum pode sumir. Numa branch nascida quinze commits
atrás a contagem batia por coincidência, e a única conferência válida foi o inventário.

O contrato do grupo diz o que este trabalho exige a mais (skills de revisão por tipo de
Task, verificação visual, harness de carga). Leia antes do primeiro parecer.

### Declare a unidade — o defeito mora um nível acima de onde te mandaram olhar

O modo de falha mais caro desta skill não é ler pouco: é ler **na unidade errada**. Antes de fechar
o parecer, diga em uma linha qual foi a sua unidade de leitura — e suba um nível:

| Você recebeu | Sua unidade mínima é |
|---|---|
| um diff | a **função inteira** onde ele caiu |
| uma função corrigida | o **arquivo**: o que as irmãs do mesmo tipo fazem (guard, flag de em-voo, limpeza na troca) |
| um módulo que fala com a rede | a **rota inteira**: para QUAL destino cada função fala, e o que a tela mostra quando cada uma falha |
| uma correção que muda **tempo de voo** | **todo mundo que corre junto com aquele voo** — a corrida nova não aparece em nenhuma linha do diff |
| um porte de padrão | a **rota de destino**: número que veio junto (teto, prazo, limiar) é medida da origem e precisa ser justificado de novo aqui |
| a correção de um defeito de família | a **branch**: `git grep` do símbolo, com a contagem no parecer |

Seis rodadas já caíram nessa forma: um teto curto copiado para uma rota que espera minutos; uma
escrita sem guard a uma linha da escrita guardada; uma função nova nascida fora da regra que as
quatro irmãs seguiam; e o defeito que atravessou a branch inteira porque cada Task olhou o próprio
arquivo. **Custo do remédio: um `git grep` de quatro segundos.**

**Rodada de CORREÇÃO tem duas perguntas fixas, e elas são espelhadas:** (1) *o que esta rodada mudou
de identidade ou de ciclo de vida — e o que passou a RE-EXECUTAR por causa disso?* Se a resposta for
"uma limpeza/desligamento destrutivo", o teste exigido é o que re-executa **durante a operação**,
não o caminho feliz. (2) *O que passou a NÃO re-executar?* Quando a correção tira algo de uma
condição ou lista de dependências, cobre a prova do **caminho que aquilo existia para servir** — o
desfecho inteiro, não "a referência aponta pro lugar novo". A primeira pega o conserto que quebra; a
segunda, o conserto que desliga. E na escolha da receita, **tirar a armadilha vence apagar o
sintoma**: rotina destrutiva não pode depender da identidade de quem a chama. Uma correção com causa
reproduzida, mutação provada e sete gates verdes já **quebrou o recurso que consertava** — um
callback novo a cada atualização de estado re-disparava a limpeza destrutiva de um efeito, o recurso
morria com a tela dizendo que funcionava, e nenhum teste do lote re-executava durante a operação. A
pergunta espelhada, no portão seguinte, rendeu quatro testes; um deles falhava **por excesso** antes
do fix.

### O teste prova o cenário, ou prova a si mesmo?

A pergunta não se responde lendo o teste. Responde-se **quebrando o código de propósito e vendo o
teste cair**:

1. Copie o subprojeto para fora do repo (o repo fica intocado — mutação por regex na árvore de
   trabalho já apagou `role`/`aria-live` numa execução, ver `executor.md`).
2. Remova **a linha da correção**, uma de cada vez, e rode a suíte.
3. Caiu só o teste novo → ele prova o cenário. Nada caiu → **aquele ponto não tem teste**, e isso é
   achado (`REGISTRADO` de lacuna, não bloqueador).

Numa mesma execução, tirar um guard derrubou exatamente a asserção do teste novo e tirar o guard do
atalho irmão deixou a suíte inteira verde — o segundo ponto virou nota de lacuna. Outra mutação
devolveu **880 testes verdes com o defeito de volta inteiro**. Não é sugestão: é a única coisa que
separa teste que prova o cenário de teste decorativo.

**E a mutação é do PORTÃO, não do executor.** Pedir a ele que rode a mutação antes de marcar o passo
é barato e ajuda — e não substitui você rodá-la em **todo teste novo que um passo ou uma receita
exigiu**: teste que nasce com o nome certo e não exercita o que promete passa por qualquer leitura,
inclusive a de quem o escreveu. A régua "rode a mutação antes de marcar" já entrou no contrato de um
grupo depois de um teste que **não importava** o módulo que dizia testar — e o mesmo defeito voltou
**duas vezes** depois dela, com o executor declarando o teste feito (um teste procurando string na
saída de um `toString()`, sem render, com a suíte verde e o estado errado em produção). Nas cinco
ocorrências daquele trabalho, quem matou o defeito foi o revisor mutando; a régua no executor não
impediu nenhuma.

**Harness fecha corrida determinística; não fecha fronteira externa.** Defeito que é **ordem de
efeitos dentro do nosso próprio código** (carregar antes de `ready`, poll, evento fora de hora) se
prova por teste que reproduz a sequência — o harness exercita a causa inteira. Defeito que depende
de **algo FORA do nosso código emitir o evento** (a plataforma, o navegador, o SO: erro de rede de
um componente nativo, permissão, teclado, câmera) **não** se prova por mock: clicar num botão do
mock prova o mock — aí a prova é no ambiente real, e produzir o desfecho de falha costuma ser barato
(modo avião, serviço derrubado). A régua se pagou na primeira aplicação: a fumaça no aparelho que
ela obrigou achou um evento de início de carga apagando a mensagem que o tratador de erro acabara de
gravar — o erro aparecia e sumia, e nenhum teste do lote veria.

**E o fixture não pode ser o mundo em que o defeito é invisível.** O teste que prova "o morto some"
usa um **vivo diferente**, nunca um mundo sem vivos: com o mundo vazio, "morto some" e "apaga tudo"
dão a mesma saída, e a suíte assina embaixo do defeito. Já foram seis chamadas de teste passando
"não há ninguém vivo" como fixture, e era o caminho de perda de dado.

**Nem um mundo que o servidor NUNCA produz.** É a forma oposta da anterior e sai mais cara: ali o
teste não podia falhar; aqui ele **afirma** um estado impossível, e a suíte verde assina embaixo.
Todo fixture que sustenta uma asserção se confere contra um dado real — um `curl` na rota, uma linha
da tabela — antes de valer como prova. Um fixture já trouxe preenchido um campo que o serviço
entrega nulo em **todos** os registros reais, com um teste afirmando o rótulo que só aquele estado
produz; apareceu em três arquivos de duas Tasks, e a segunda ocorrência reprovou uma rodada.
**Fixture corrigido entra no MESMO commit da correção que ele trava** — senão a
trava é ilusória.

#### Antes de aceitar uma bateria de sabotagem, quatro conferências

Esta seção já cresceu porque cada modo de falha dela custou uma rodada. As quatro abaixo são as que
se esquecem na hora, então elas ficam juntas, em lista, e não em prosa:

1. **O corte isola o caminho que a afirmação NOMEIA?** Pergunta que invalida: *existe outra
   explicação para esses testes terem caído?* Corte que cai em código compartilhado — um mapeador
   de erro, um `catch` comum, um helper — derruba todos os caminhos juntos e é compatível com
   hipóteses opostas: ele prova que algo ali é exercitado, não a frase escrita ao lado. Um corte num
   mapeador compartilhado entre gravar e excluir já sustentou tanto "o teste novo fecha um buraco"
   quanto "é cópia do irmão"; refeito desligando só o caminho afirmado, caíram exatamente os dois
   testes certos, com o resto como controle. **Afirmação verdadeira com prova incompleta: complete a
   evidência, não reprove** — reprovar aí cobra do executor o desenho da prova, não o trabalho.
2. **Cada corte diz QUAL ARQUIVO DE TESTE acusou?** "Derrubou 7 testes" não separa *a trava nova
   mordeu* de *um teste antigo já mordia e a trava nova leva o crédito*. Duas baterias da mesma Task
   saíram sem o campo; refeitas com ele, **10 de 10** cortes que mordiam tinham como acusador uma
   das travas novas — a suspeita era a certa de se ter, e só se soube porque o campo existia. **E o
   denominador é único na bateria inteira**: cortes medidos contra suítes de tamanhos diferentes não
   se comparam. Corte que derruba o processo (estouro de pilha) roda **com filtro no teste que se
   afirma acusador** — sem filtro o log sai em megabytes e o acusador só aparece lendo pilha; com
   filtro, o próprio comando o nomeia.
3. **Cada corte vem em PAR com um controle?** O corte responde "o defeito volta?"; o controle
   responde "e a função continua viva?". Sem o segundo, um conserto que destrói a funcionalidade —
   ou que "conserta" apagando a linha — passa como sucesso. Medido três vezes: um controle separou
   "o guard funciona" de "o campo está morto"; outro, acrescentado fora da receita, derrubou as
   quatro provas de uma vez e mostrou que remover a linha não passa; um terceiro manteve verde o
   caminho que já funcionava. **Controle que fica VERDE sob o corte é o desenho certo, não uma
   lacuna** — controle que morde virou uma segunda trava, e o arquivo perdeu a única medida de "não
   recusa demais" que tinha.
4. **Bloqueador sobre AUSÊNCIA foi medido por dois caminhos?** "Este ponto não tem teste", "esta
   função não tem caller", "o campo não existe" são respostas a **uma** pergunta específica, e
   perguntas diferentes sobre o mesmo repositório devolvem respostas opostas. Escreva no parecer
   **qual pergunta a busca fez**, e refaça por um segundo caminho antes de virar bloqueador. Uma
   ausência registrada por dois dias já caiu na primeira busca feita com outra palavra.

**A outra metade: receita que instala TRAVA exige prova invertida.** A mutação responde "o teste
prova o cenário?"; a prova invertida responde "a trava trava?". Toda receita cujo objetivo é impedir
regressão futura — tornar prop obrigatória, apertar um tipo, acrescentar um lint — só vale entregue
com a verificação **vermelha sem a correção** e verde com ela. Peça as duas ao executor, em disco, e
leia as duas. Sem isso, ou a trava nasce vermelha pra sempre e alguém a desliga, ou ela passa por
trava sem travar nada.

Numa aplicação real a trava eram duas linhas mais a prova invertida; ligada, ela acusou **dois**
erros, revelando um segundo ponto sem a prop que a receita não previa — a receita dizia duas linhas
e eram três. **Ao receitar uma trava, rode a verificação com a mudança aplicada ANTES de escrever o
número de passos** — contar os pontos que você já conhece não é contar os pontos.

### Prova ao vivo mede o que está SERVIDO, não o que está commitado

**Buildar não é prova. Prova é casar o identificador do artefato que você acabou de construir com o
que a página realmente carregou** — o hash do bundle, a data do arquivo, o que a plataforma tiver.
Buildar é o primeiro passo; o segundo é conferir, e é ele que vale. Descubra antes **o que a porta
serve** — o comando que o serviço realmente executa: porta de desenvolvimento servindo *build*
estático mostra o commit anterior sem avisar ninguém. **Vale igual para serviço de BACKEND de
longa duração: ele serve o código de quando subiu** — confira o início do processo contra a data do commit,
ou suba instância própria em outra porta (e nunca reinicie o serviço do usuário para medir). Um
processo no ar desde antes do commit medido já virou quase um falso "bloqueador aberto".

O mesmo defeito já apareceu por dois mecanismos no mesmo dia: uma porta servindo build pré-compilado
entregou um bundle anterior ao commit — custou **três** medições refeitas e uma prova de parecer que
teve de ser retirada; e, já com o build feito antes, um service worker serviu o `index.html` do
próprio cache, e portanto o código anterior. Nas rodadas seguintes todo parecer trouxe o par
conferido — o identificador do artefato que o build acabou de gerar e o que a página carregou — e
nenhuma medição precisou ser refeita. A receita concreta de como conferir é **do repositório**, não desta skill: ela vive no
arquivo de regras do grupo, com o comando daquele projeto.

**Antes de abrir o navegador, compare as expressões lado a lado.** Na revisão de uma branch, duas
medições foram gastas caçando um defeito pelo caminho errado; as três expressões da mesma derivada,
postas termo a termo, mostravam o lugar em minutos.

### Meça nos dois hosts e nos dois estados, e diga em qual

Tela que existe em dois hosts (celular e computador, painel e modal) mede-se **nos dois** — e o
parecer diz em qual largura cada número foi tirado. Três rodadas de uma execução caíram por medição
num breakpoint só: a aba nova aparecendo no computador quando a Task era do celular, e a mesma aba
sumindo dos dois lugares na faixa intermediária.

O eixo não é só a largura: é **qualquer estado da região vizinha**. Duas outras rodadas caíram por
medir no estado errado — a rolagem conferida numa aba que rolava por ter dezenas de itens, quando o
número original viera de outra; e o teto de um painel calibrado só com o vizinho **aberto**, quando
o estado normal dele é fechado. Regra curta: **meça sempre no mesmo estado em que o número original
foi levantado, e anote o estado junto do número.**

**Prova de comportamento vai até o desfecho.** "Conectou", "salvou", "abriu" — não o estado
imediatamente anterior a ele. Print de botão habilitado não é prova de que o clique funciona: uma
evidência que parou no botão desabilitado obrigou o portão a rodar o fim do fluxo.

## E o tom de tudo isto

A revisão é adversarial: você tenta **quebrar** o estado final, não confirmar que o plano foi
seguido. Parecer que só confirma plano, tipos e build é o portão não existindo.

