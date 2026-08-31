# Árbitro — o lançamento (fase 2)

Esta página é do **momento em que o time nasce**: escolher contas, conferir ferramental e abrir as
sessões. Você a lê **uma vez**, antes da Task 1, e não volta a ela — salvo para abrir uma sessão
nova no meio (rotação, substituição).

Volte para `arbitro.md` assim que o time estiver de pé.

## Antes do time: leia a política de contas da máquina

**A política mora em `~/.hangar/orquestracao-contas.md`, não aqui.** Leia antes de abrir a primeira
sessão e **copie pro contrato só o que este trabalho vai usar**, na tabela `## Quem é quem` das
regras. Não repasse o arquivo inteiro: sessão escolhe pelo que está no contrato.

Três regras de leitura, e as três protegem a mesma coisa — a fatura de quem confiou em você:

- **Vale a tabela, não a prosa.** O arquivo tem uma tabela de contas liberadas, gravada pelo painel
  quando o usuário liga ou desliga uma conta pela tela. **Conta fora da tabela é proibida**, mesmo
  que um parágrafo abaixo pareça permitir: prosa envelhece, a tabela é o que ele mexeu por último.
- **Conta que cobra por token é proibida.** Você descobre que uma conta existe; só o usuário sabe
  se ela debita. Discovery lista provider, modelo e endereço — nada disso diz de quem é a conta nem
  se ele quer gastar ali. Provider novo que apareceu desde a última revisão **não entra por conta
  própria**. Numa máquina real, 341 dos 390 modelos do catálogo eram de um provider pago por token.
- **Arquivo ausente ou velho → monte o inventário e faça UMA pergunta** (quais podem, quais são
  assinatura, quais cobram), escreva a resposta lá com a data, e **não abra sessão nenhuma** até
  ela chegar — nem "só pra testar". A receita de levantamento está dentro do próprio arquivo.

**Recado "a configuração de modelos do grupo mudou no painel"** vem da tela, não de uma sessão: a
linha já está no `regras-<gid>.md`. Releia o arquivo e aplique — trocar conta ou modelo **é** fechar
e abrir, o Claude não troca com a sessão aberta:

| A sessão daquele papel está… | O que fazer |
|---|---|
| parada | feche e abra outra já na configuração nova |
| trabalhando | deixe terminar; a **próxima** nasce na nova. O contexto dela vale mais que o modelo |
| é você (árbitro) | termine a tarefa e passe o bastão pelo rito de "Sucessão do árbitro" |

Responda o recado só se ele pedir.

## Levante o ferramental ANTES de abrir o time

Sessão nova não sabe o que a máquina tem. Se você não disser, cada uma revisa e constrói pelo
método que inventar — foi o que aconteceu numa execução real: o revisor achou três bloqueadores de
verdade **sem usar nenhum** dos subagentes de revisão instalados, porque o contrato tinha deixado
essa parte em branco.

Uma varredura, uma vez, no começo. Depois **escreva no contrato uma tabela por tipo de trabalho** —
quais subagentes e skills o revisor despacha, e quais ajudam o executor a entregar. Cada sessão nova
recebe isso pronto, em vez de descobrir sozinha (ou não descobrir).

Olhe as três prateleiras: **subagentes** (revisores por linguagem e por dimensão — falha silenciosa,
segurança, acessibilidade, cobertura de teste), **skills** (auditoria de caminho de clique, revisão de
segurança, prontidão pra produção, QA de navegador, padrões da casa) e **comandos** do marketplace.

E passe cada uma pelas **três perguntas** do `SKILL.md` ("Ferramenta de fora — skill, subagente,
comando"): existe com esse nome, serve ao fluxo, serve aos arquivos. As três já erraram aqui na
mesma varredura — uma ferramenta anunciada como skill que era comando, a mesma montando o diff de
mudanças não commitadas num portão que revisa commit feito, e um revisor por linguagem cujo filtro
de extensão não enxergava o tipo de arquivo onde moravam os dois bloqueadores de tela do trabalho.

Ferramenta que não passa nos três: registre no contrato **por que não serve**, com uma linha. Isso
vale tanto quanto a lista do que usar — evita que a próxima sessão gaste turno tentando.

## Abrir uma sessão — receita, não decisão

**Exceção:** a **sessão verificadora do revisor** não é sua. Ele abre, dirige e fecha sozinho, sem
te pedir — é braço dele pra rodar app, clicar tela e capturar print, e o que chega em você continua
sendo só o parecer. Não crie, não gerencie e não cobre relatório dela. **O modelo dela não é escolha
dele**: sai do contrato, como o de todo mundo — mas quem cria e confere é ele, não você.

### Papel com rodízio: qual linha da tabela vale para esta Task

A tabela `## Quem é quem` ganha uma sétima coluna, `vez`, **só** quando algum papel reveza entre
contas. Sem rodízio, ela não existe e nada aqui muda.

Papel com `vez` numérica tem mais de uma linha, uma por conta, e **a Task N cabe à linha de índice
`(N-1) % total`** daquele papel, na ordem da tabela. Ninguém decide de quem é a vez — é aritmética
sobre o número da Task, e é por isso que duas sessões que fazem a conta separadas chegam ao mesmo
resultado sem combinar nada. Os dois erros fáceis: é `(N-1)`, não `N` (a Task 1 usa a **primeira**
linha), e a volta reinicia — com 3 contas, a Task 4 é da primeira de novo, não a continuação da 3.

O rodízio **não** é paralelismo: numa Task existe **uma** sessão daquele papel, na conta da vez.
Rodar Tasks ao mesmo tempo é outro mecanismo — worktree por Task, declarado no PLANO, em
`paralelo-worktree.md` —, e a regra "uma rodada, UM revisor" vale inteira nos dois casos.

Vale para toda sessão que você cria. Os cinco passos são **uma unidade**: o turno não fecha
no meio deles.

1. **Criar na conta padrão do agente:** `hangar-send --new <nome> <cwd>`, **sem** `--engine`.
   Motor de provedor entra **só** quando o plano nomeou um: `--engine <motor>`.
   *"Sessão de <agente>"* quer dizer a conta padrão dele. Modelo daquele fabricante
   acessível por gateway, roteador ou API **não é** uma sessão dele — é outro provedor
   servindo um modelo parecido, com outra conta e outro comportamento.

   **Modelo, esforço e permissão vão NO PRÓPRIO `hangar-send --new`** (desde 25/08/2026):
   `--model <id>`, `--effort <nivel>` e `--permissao <modo>`. O contrato que nomeia modelo e
   thinking (o caso normal quando o time roda em Pi) cabe no comando — a sessão já nasce nele:

   ```bash
   hangar-send --new <nome> <repo> --provider pi --model <provider>/<id> --effort <nivel>
   ```

   No Pi o `--effort` vira `--thinking` (aceita também `off|minimal`); no Kimi só `--model`;
   `--permissao` é só Claude. O backend valida **antes** de qualquer efeito em disco: modelo fora
   da regex, nível fora da lista fechada ou provider desconhecido devolvem 400 e a sessão **não
   nasce** — nunca uma sessão que parece estar no modelo certo e não está. O caminho alternativo
   (criar sem os flags e trocar depois por `/cp-model` + `/cp-think`) funciona, mas deixa a sessão
   viva um intervalo no modelo errado, e contradiz o passo 2 abaixo. (Instalação com `hangar-send`
   antigo, sem os flags: o POST direto na API com `model`/`effort`/`permission_mode` no corpo
   continua valendo como plano B.)

2. **Provar o que nasceu**, lendo o motor/modelo **real** da sessão, nunca o que você pediu.
   Divergiu do plano → apague e recrie. Sessão errada recebendo o pedido é trabalho inteiro no lugar
   errado, e o dado que denuncia isso aparece antes de qualquer erro.

   Duas provas, e você quer as duas — elas falham por motivos diferentes:

   ```bash
   tmux display -p -t "=<nome>:" '#{pane_start_command}'   # o argv real com que o pane subiu
   ```

   Isso mostra `exec pi --session-id … --model <provider>/<id> --thinking <nivel>` e prova que o
   **pedido** virou comando. Não prova o que o agente **aceitou**: o Pi trunca o nível ao que o
   modelo suporta, então peça também a prova **ao vivo** à própria sessão (statusline ou o retorno de
   `/cp-think`) no primeiro turno dela, antes do primeiro `Edit`. Repetir o que o kick-off pediu não
   é prova.

   Não leia `/proc/<pid>/cmdline` esperando as flags: o Pi reescreve o próprio argv e o cmdline
   mostra só `pi`. Isso já pareceu, por um minuto, uma sessão criada sem modelo nenhum.

   **E prova de modelo prova o modelo, não o HARNESS.** Uma sessão Claude Code com motor apontando
   pro provedor X e uma sessão Pi rodando o modelo X mostram **a mesma linha de status**. Quem
   distingue é o `pane_start_command` (`claude` × `pi`) e o `provider` que a API devolve — confira os
   dois. Medido em 15/08/2026: três executores nasceram na forma errada e mesmo assim provaram
   modelo e esforço corretamente antes do primeiro `Edit`; custo zero só porque as worktrees ainda
   estavam limpas.

   Junto: **prova por sidecar de status tem que casar o `session-id` com o da sessão viva** — o
   diretório guarda um arquivo por id e não os apaga quando a sessão morre. Dois daqueles três
   leram o sidecar da sessão morta que ocupava o pane antes, e o valor saiu certo por coincidência.
3. **Escrever o pedido num arquivo** e entregar com `hangar-send <nome> "$(cat <arquivo>)"`.
   Pedido longo digitado direto na linha quebra: `|`, `$`, crase e `|` de "SIM | NÃO" viram
   comando, e a mensagem sai mutilada ou não sai.
4. **Conferir o retorno.** `entregue -> <nome>` é entrega. Qualquer outra coisa — `404`,
   erro de uso, silêncio — é **não entregue**: reenvie, não siga em frente.
   **E `entregue` prova entrega, não EXECUÇÃO.** Antes de registrar (ou reportar) que a sessão
   está trabalhando, confira o engajamento: o ctx dela saiu do zero na statusline, ou o pane está
   processando. Sessão que recebeu o kick-off e morreu no timeout do provedor fica `idle` com a
   mesma cara de sessão parada — medido em 20/08/2026: ctx parado em 1k/1M com `Retry failed
   after 3 attempts` no pane, e a Task reportada como "rodando"; quem viu foi o usuário. No
   reenvio, aponte só o CAMINHO do kick-off (pegou: ctx foi de 1k a 109k em um minuto — essa é a
   prova barata).
5. Só então o turno fecha. **Sessão aberta com pedido não entregue é uma sessão que ninguém
   vai usar** e que você vai achar que está trabalhando.

