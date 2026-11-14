---
titulo: Navegador da sessão
categoria: sessoes
resumo: Um navegador dentro do app, que você e o agente usam juntos.
icone: globe
ordem: 30
onde: desktop
---
Cada sessão tem um navegador de verdade dentro do app. Serve para ver o site do projeto rodando no
seu computador sem trocar de janela, e o agente usa o mesmo navegador para testar o que fez.

## Abrir o navegador

No painel ao lado da conversa, clique em **+** e escolha **Navegador**. Digite o endereço na barra,
por exemplo `localhost:3000`. No Linux, o navegador precisa do Google Chrome ou do Chromium instalado;
sem eles, a opção aparece desligada com o motivo.

## O agente usa o mesmo navegador

Quando o agente abre uma página, clica ou preenche um formulário, é neste navegador. Você vê tudo
acontecendo e pode mexer junto. Não há abas: é um navegador por sessão.

## Mostrar um pedaço da página ao agente

Clique no botão de escolher elemento, na barra do navegador, e depois no que você quer mostrar. O
elemento vai para a caixa de mensagem, com um recorte da tela, e você escreve o que quer que o agente
faça com ele. Shift+clique escolhe mais de um; Esc cancela. Enquanto escolhe, o clique não chega à
página.

## Usar o seu login

O navegador da sessão começa sem login em nenhum site. Para não precisar entrar de novo:

1. Abra o site no navegador da sessão.
2. Clique em **Trazer meu login**, à direita da barra de endereço.
3. Clique em **Trazer login**. A página recarrega já com o seu login.

Só vêm os cookies do site; senhas e histórico ficam no seu navegador. No Windows, na primeira vez, o
app mostra o passo a passo para ligar uma opção no seu Chrome ou Edge.

O agente também pode pedir o seu login num site. O pedido aparece no app com **Permitir** e
**Recusar**, e só o seu clique aprova.

## Cuidados

- O login trazido vale no navegador de todas as sessões desta máquina e para qualquer agente que o
  use. Traga só o site de que o trabalho precisa. Marcar **Todos os sites** deixa os agentes logados em
  tudo em que você está logado.
- O Google (Gmail, Drive, YouTube logado) recusa este navegador. Para esses, use o seu navegador
  normal.
- Para sair de um site, saia da conta nele, no próprio navegador da sessão.
