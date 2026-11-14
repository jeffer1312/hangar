---
titulo: Tarefas agendadas
categoria: sessoes
resumo: Deixe um pedido para o agente executar numa data, em dias da semana ou em intervalos.
icone: clock
ordem: 20
onde: todos
ancora: scheduled-tasks
---
Uma tarefa agendada manda um pedido ao agente na hora que você escolher. Ela roda mesmo com o app
fechado, enquanto a máquina escolhida estiver ligada.

## Criar uma tarefa

1. Abra **Configurações › Tarefas agendadas** e escolha o servidor.
2. Clique em **Nova tarefa**.
3. Dê um nome e escreva em **O que o agente deve fazer?** o pedido que vai a cada execução.
4. Em **Quando executar**, escolha **Uma vez** (data e horário), **Dias da semana** ou **Intervalo**.
   O horário é o da máquina em que a tarefa roda; o formulário mostra qual é.
5. Clique em **Criar tarefa**.

## Onde ela roda

- **Nova conversa a cada execução**: você escolhe a pasta do projeto, o agente, a conta e o modelo.
  Ligando **Criar uma worktree para cada execução**, cada vez trabalha numa cópia própria do projeto.
- **Uma conversa existente**: o pedido vai para uma conversa aberta, com o agente e a conta dela. Se
  ela estiver ocupada, o pedido entra na fila. Se a conversa for fechada ou recomeçar do zero, a
  tarefa não envia e pede um novo destino.

## Acompanhar e mudar

A lista mostra a próxima execução, a última tentativa e **Abrir conversa** para ver o que o agente
fez. Nas ações de cada tarefa você edita, pausa, ativa, executa agora ou exclui.

- **Pedido enviado** quer dizer que o agente recebeu o pedido, não que terminou o trabalho.
- Pausar não interrompe um pedido que o agente já recebeu.
- Excluir a tarefa mantém as conversas que ela já criou.

## Com a máquina desligada

Se a máquina estava desligada no horário, a tarefa tenta uma vez quando ela volta, sem repetir todos
os horários perdidos. Uma tarefa marcada **Entrega não confirmada** não é reenviada sozinha: confira
a conversa antes de usar **Executar agora**.
