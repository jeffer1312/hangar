---
titulo: Conectar o celular
categoria: comecar
resumo: Pelo Hangar Connect, de qualquer rede, ou pelo QR na mesma rede ou no Tailscale.
icone: smartphone
ordem: 10
onde: todos
---
O app do celular mostra as sessões do seu computador e deixa você conversar com elas. Escolha um dos
caminhos abaixo. Dá para usar mais de um no mesmo celular.

## Pelo Hangar Connect

Funciona no 4G e em qualquer Wi-Fi, sem VPN e sem mexer no roteador.

1. No computador, abra **Configurações › Connect**.
2. Entre com a sua conta do Hangar Connect, ou crie uma, e dê um nome a este computador. É com esse
   nome que ele aparece no celular. Clique em **Entrar e ligar** (ou **Criar conta e ligar**).
3. No celular, na tela de entrada, toque em **Entrar com o Hangar Connect**, entre na mesma conta e
   escolha a máquina.
4. O pedido aparece no computador. Confira se o código do celular é o mesmo nos dois e aprove.

O pedido vale por até 10 minutos. Passou disso, peça de novo pelo celular.

## Pelo QR, na mesma rede ou no Tailscale

1. No computador, abra **Configurações › Servidores** e clique em **Parear um celular**.
2. Em **Endereço no QR**, escolha o da rede local para usar em casa, ou o do Tailscale para usar de
   fora de casa.
3. No celular, toque em **Escanear QR** na tela de entrada e leia o código.
4. Confira se o código escrito embaixo do QR é o mesmo que aparece no celular.

Para usar o Tailscale, instale o app dele no celular e entre com a mesma conta do computador. O QR dá
acesso completo à máquina: não deixe ninguém fotografar a tela.

## Com usuário e senha

Se você já usa a sincronização entre aparelhos, toque em **Entrar com usuário e senha** na tela de
entrada do celular e use o mesmo usuário e senha dela. As máquinas liberadas para a sua conta aparecem
sozinhas.

## Tirar o acesso de um celular

No computador, abra **Configurações › Connect** e, em **Celulares com acesso**, clique em
**Tirar acesso** no aparelho. Isso fecha só a entrada pelo Connect: um celular que também entrou pelo
QR continua entrando pela rede local e pelo Tailscale.

## Quando não conecta

- "Não consegui falar com…" num endereço do Tailscale: confira se o Tailscale está ligado no celular,
  na mesma conta do computador, e se o computador está ligado.
- A máquina aparece como **Fora do ar** no Connect: ligue o computador e espere o Hangar subir.
- O pedido do Connect sumiu antes de você aprovar: ele venceu. Peça de novo pelo celular.
