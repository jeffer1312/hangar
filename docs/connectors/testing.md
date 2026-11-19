# Testar um conector

## Host isolado, sem servidor

`--registry <arquivo>` cria um host só para o comando, com catálogo próprio. Não sobe backend, não
adota sessões, não instala hooks e não abre porta. Ao fim do comando, o processo do conector é
encerrado.

```bash
cd crates
cargo build -p hangar-server
REG=$(mktemp -d)/connectors.json
target/debug/hangar-server connectors --registry "$REG" add ../examples/connectors/work_tracking.json
target/debug/hangar-server connectors --registry "$REG" tickets js-demo --limit 2
target/debug/hangar-server connectors --registry "$REG" ticket js-demo c0ffee-01
```

O exemplo em JavaScript precisa de `node` no PATH e devolve tickets fictícios, sem credencial nem
rede. Um conector compilado pode usar um executável próprio; o Hangar não instala interpretadores.

A revisão (`work_review/v1`) tem um exemplo próprio, com um ticket em cada estado
(`rev-running`, `rev-waiting`, `rev-done`). Os comandos `review*` só funcionam com `--registry`:

```bash
target/debug/hangar-server connectors --registry "$REG" add ../examples/connectors/work_review.json
target/debug/hangar-server connectors --registry "$REG" review js-review rev-waiting
target/debug/hangar-server connectors --registry "$REG" review-asset js-review rev-waiting <review_id> <asset> > lado.png
target/debug/hangar-server connectors --registry "$REG" review-decide js-review rev-waiting \
  '{"review_id": "<review_id>", "decisions": [{"id": "47", "choice": "igualar"}]}'
target/debug/hangar-server connectors --registry "$REG" review-decide js-review rev-waiting \
  '{"review_id": "<review_id>", "decisions": [{"id": "48", "choice": "manter", "reason": "sem legenda no kit"}, {"id": "49", "choice": "remover"}]}'
```

Cada comando abre um processo novo do conector. Para a prova atravessar comandos, o exemplo guarda
as decisões enviadas em `hangar-work-review-demo.json`, na pasta temporária do sistema (ou no caminho
de `WORK_REVIEW_DEMO_STATE`); apague o arquivo para recomeçar. O `rev-waiting` tem três decisões e
aceita lotes: depois do primeiro `review-decide`, a leitura continua em `waiting` com as duas
restantes e o mesmo `review_id`. Depois da última, a leitura seguinte vem em `applying` e as outras
em `done`. Repetir um lote já enviado responde ok.

## Conector do fornecedor

O fornecedor entrega o programa, o manifesto e o procedimento de login. Instale a versão adequada
ao sistema da máquina do servidor e confira os caminhos do manifesto antes do cadastro. Caminhos
relativos partem da pasta do manifesto; nome simples de executável usa o PATH do servidor.

Prove primeiro com `--registry <arquivo temporário>`: cadastro, lista com paginação e detalhe de um
ticket conhecido. Se oferecer `work_actions/v1`, confira também o catálogo pelo endpoint
`GET /api/connectors/{id}/actions?ticket_id=<id>` depois de cadastrar no servidor.

Credenciais ficam no mecanismo do conector, nunca no manifesto nem em argumentos. Sem login ou
com token recusado, o conector responde `auth_required`; siga o login do fornecedor e tente de novo.

## Em outra máquina

1. Instale o Hangar e o conector para o sistema daquela máquina, seguindo a distribuição de cada um.
2. Faça o login do fornecedor naquela máquina, quando necessário.
3. Prove primeiro no host isolado: `tickets <id> --limit 5`, a página seguinte pelo `next_cursor` e
   `ticket <id> <chave>`.
4. Cadastre no servidor: `hangar-server connectors add <manifesto>`. Repita as consultas sem
   `--registry`, ou abra Tickets no app nativo.
5. Para remover o cadastro: `hangar-server connectors remove <id>`. A remoção encerra o processo;
   a desinstalação do programa segue as instruções do fornecedor.

O Hangar não substitui o executável do manifesto por um conector interno. Atualizações do conector
continuam sob a distribuição do fornecedor, respeitando as versões das capacidades negociadas.
