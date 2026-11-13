---
titulo: Cofre de senhas
categoria: ferramentas
resumo: Guarde senhas e tokens para os agentes usarem sem o valor passar pela conversa.
icone: key-round
ordem: 10
onde: todos
---
O cofre guarda senhas e tokens criptografados numa das suas máquinas. Você vê e copia quando
precisar, e o agente usa o que está lá para entrar num site ou rodar um comando sem a senha aparecer
na conversa.

## Escolher onde o cofre fica

Abra **Configurações › Cofre**. Em **Onde fica o cofre**, escolha uma das opções:

- Guardar o cofre nesta máquina: ele nasce vazio aqui, com a chave no chaveiro do sistema dela.
- Usar o cofre de outra máquina: esta passa a pedir as senhas à máquina dona.

Um cofre só serve todas as suas máquinas. A máquina dona precisa estar ligada para as outras usarem
as senhas.

## Guardar uma senha ou token

1. Em **Configurações › Cofre**, toque em **Nova senha** (no celular, **Adicionar senha ou token**).
2. Dê um nome e preencha a senha ou o token. Para um token, deixe usuário e site em branco.
3. Em **Uso por agente**, escolha a regra:
   - **Pedir sempre**: cada uso por um agente pede sua aprovação, aqui e no celular.
   - **Liberada**: o agente usa sem perguntar.
4. Toque em **Salvar**.

Mostrar e copiar uma senha não pedem confirmação.

## Quando o agente pede uma senha

O pedido aparece dentro da conversa de quem pediu, logo acima da caixa de mensagem. Ele diz quem pede,
qual senha, para quê e o motivo.

- Para usar uma senha guardada: **Permitir** ou **Negar**.
- Para uma senha que ainda não está no cofre: digite-a e escolha **Guardar no cofre**, **Só desta
  vez** ou **Recusar**.

Quem responder primeiro, no computador ou no celular, fecha o pedido nos dois.

## Cópia de segurança

Em **Cópia de segurança**, escolha outra máquina para receber uma cópia criptografada todo dia, e
grave uma **Senha de recuperação**. Só ela abre a cópia: guarde-a fora do Hangar, num papel por
exemplo. Se a máquina dona se perder, **Restaurar de uma cópia** traz as senhas de volta para um
cofre vazio.

## Quando o cofre não abre

- Chaveiro trancado: a máquina dona ligou com o chaveiro do sistema trancado. Destranque pelo aviso
  na lista de sessões.
- A máquina dona não respondeu: ligue-a ou confira a conexão com ela.
- Escolheu como dona uma máquina sem cofre: deixe de usar o cofre dela ou guarde o cofre nesta.
