# Executor — Task de fluxo

Esta página é da **Task que cria ou muda orquestração** — tmux, CLI, processo, conta, rede. Vale
mesmo que o plano não tenha o passo de fumaça: plano incompleto não é permissão pra pular. Se a sua
Task não é dessas, ela não é sua; o ciclo está em `executor.md`.

## Você tem que RODAR o fluxo

**O duplo de uma primitiva devolve o que a PRIMITIVA devolve.** Fake que reproduz a sua suposição
sobre o tmux prova a suposição, não o tmux. Uma Task já entregou com mais de três mil testes verdes
e o fluxo inteiro morto — **405 linhas de teste novo passavam com o módulo inoperante**, porque os
fakes assumiam que o nome pedido era o nome da sessão tmux (não era) e nenhum teste exigia o Enter.
Dez bloqueadores, achados pela revisora rodando contra a fonte real.

- Antes do commit, **rode o fluxo de ponta a ponta contra a fonte real** — o tmux de verdade, a CLI
  de verdade, a conta de teste de verdade — e cole no reporte o que aconteceu, não o que os testes
  dizem que aconteceria.
- **Contagem da suíte que CAI vira nota obrigatória no reporte.** Uma unidade a menos que a base é
  meio relato: na mesma Task, sete testes de uma Task aprovada tinham sido apagados, calados.

A régua tem duas metades, e a segunda foi a mais cara desta skill até hoje:

1. **O duplo substitui a I/O, nunca a função sob correção.** Um rótulo de conta usado como caminho
   de diretório já sobreviveu a **duas rodadas de suíte verde**, porque o duplo reproduzia a
   suposição do código em vez de conferi-la.
2. **Teste que troca a biblioteca inteira por um duplo prova que o botão chama a função — nunca
   para onde a função vai.** Três arquivos de teste trocando as bibliotecas de rede por duplos já
   fizeram os portões de **cinco Tasks** aprovarem uma tela que promete um servidor e age noutro —
   apagando conta e conversas na máquina errada, e mandando a credencial de login para o host
   errado. O teste que faltava tem três casos e nasceu em vinte minutos. E o gate de tipos **também
   não pega**: mutar o corpo de uma função de volta para o cliente errado deixa o repositório
   inteiro com zero erros.

Daí a régua de forma: **Task que muda destino, credencial ou alvo entrega um teste com as
bibliotecas reais**, e o melhor formato é com **controle interno** — a tela vizinha que já acerta,
medida no mesmo teste. Foi assim que a revisão de conjunto provou o bloqueador em vez de argumentá-lo.

E duas réguas de desfecho, da mesma família:

- **Prova de fluxo de duas pontas é o conteúdo dos dois lados** (os dois arquivos, os dois
  identificadores), **nunca o selo que a própria tela pinta**. Um selo chumbado mostrou `✗` **verde**
  dentro do print entregue como "desfecho ok", e o defeito real era a volta perguntando ao servidor
  errado sobre ele mesmo. Dois `cat` de vinte segundos teriam poupado a rodada.
- **A evidência tem de trazer o que distingue os dois caminhos.** Prova de "foi para o servidor
  certo" traz **qual era o ativo naquele instante** — senão ela não separa "foi para o dono" de "o
  ativo já era o dono". Logs do "depois" mostrando a chamada que só sai para o ativo, dezenas de
  vezes num lado e nenhuma no outro, não provavam nada; quem separou foi um teste de componente com
  os dois lados invertidos.

