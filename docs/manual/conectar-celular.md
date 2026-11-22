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
3. No celular, a tela de entrada já abre no Connect: digite a conta e a senha e toque em **Entrar**.
   Não precisa de endereço.
4. Da primeira vez, o celular espera ser liberado. Em qualquer aparelho ou computador que já está na
   conta, abra **Aparelhos da conta**, confira se o código é o mesmo e toque em **Liberar**. Vale uma
   vez, para todas as máquinas da conta.
5. Marque as máquinas e toque em **Entrar**.

O computador entra do mesmo jeito: na primeira tela do app, entre com a conta do Connect.

## Convidados

Um convidado entra com usuário e senha próprios e vê só as máquinas e a pasta que você liberar.

1. No computador ou no celular, abra **Configurações › Convidados** e toque em **Adicionar convidado**.
2. Escolha o usuário (precisa ter `.` ou `@`, por exemplo `ana.silva`), a senha, as máquinas e a pasta
   de cada uma.
3. Passe o usuário e a senha ao convidado. Ele abre o app, digita os dois na tela de entrada e toca em
   **Entrar**, sem endereço.

Convidado criado antes desta versão aparece em **Contas antigas**: toque em **Trazer para o Connect** e
entre uma vez com o usuário e a senha da sua sincronização. A senha do convidado continua a mesma.

## Pelo QR, na mesma rede ou no Tailscale

1. No computador, abra **Configurações › Servidores** e clique em **Parear um celular**.
2. Em **Endereço no QR**, escolha o da rede local para usar em casa, ou o do Tailscale para usar de
   fora de casa.
3. No celular, toque em **Escanear QR** na tela de entrada e leia o código.
4. Confira se o código escrito embaixo do QR é o mesmo que aparece no celular.

Para usar o Tailscale, instale o app dele no celular e entre com a mesma conta do computador. O QR dá
acesso completo à máquina: não deixe ninguém fotografar a tela.

## Com usuário e senha

Se você já usa a sincronização entre aparelhos, toque em **Outras formas de entrar › Endereço e
token** na tela de entrada do celular, escolha usuário e senha e use os mesmos da sincronização. As máquinas liberadas para a sua conta aparecem
sozinhas.

## Tirar o acesso de um celular

Em **Aparelhos da conta**, no computador ou num celular já liberado, toque em **Tirar acesso** no
aparelho. Ele sai do Connect em todas as máquinas da conta em até um minuto. Um celular que também
entrou pelo QR continua entrando pela rede local e pelo Tailscale.

## Quando não conecta

- "Não consegui falar com…" num endereço do Tailscale: confira se o Tailscale está ligado no celular,
  na mesma conta do computador, e se o computador está ligado.
- A máquina aparece como **Fora do ar** no Connect: ligue o computador e espere o Hangar subir.
- O pedido do Connect sumiu antes de você aprovar: ele venceu. Peça de novo pelo celular.
