# Parte 6, grupos: gestão de grupos no Rust, grupos de N sessões em M máquinas

Status: ready-for-human (desenho para aprovação; D1 e D2 decididas; nada implementado)

Escrito em 08/10/2026 a partir de `9072e9aa6` (main). Linhas citadas conferidas nesse commit; cada
Task do plano reconfere as dela antes de codar. Plano: [`plano.md`](plano.md). Revisado por um
passe adversarial (crítica do desenho + conferência factual); as correções estão incorporadas.

Pedido do dono: migrar a gestão de grupos do Python para o `hangar-server`, melhorar o que o Python
faz mal, permitir grupo de várias sessões em várias máquinas e, no desktop nativo, mostrar cada
grupo como **um** grupo com todos os membros conhecidos, de qualquer máquina, indicando a máquina de
cada um sem partir o grupo e sem confundir nomes iguais em máquinas diferentes.

Insumo lido e não tomado como contrato: `docs/superpowers/specs/2026-09-21-grupo-pareamento-cross-server.md`
(ignorado pelo Git, nunca implementado). Cada exclusão dele foi reavaliada na seção 5.

---

## 1. Estado atual

### 1.1 Dados

Uma pasta só por backend, para todas as contas: `<projects_dir>/../.hangar-pair/`, normalmente
`~/.claude/.hangar-pair/` (`backend/app/pair.py:49-52`). O Rust lê a mesma pasta por
`HANGAR_LIST_DIRS["claude"]` (lida em `crates/hangar-server/src/list/bridge.rs:68`, usada em
`list/links.rs:97`).

| Arquivo | Quem escreve hoje | Quem lê |
|---|---|---|
| `<nome-sanitizado>.json` por membro LOCAL: `{peers, task, gid, harness, orq?}` | `pair.py` (lock `_LOCK` de processo) | `pair.py`, `links.rs` (lista do Rust), `hooks/pair_hook.py` (SessionStart, direto do disco), `orq_context.py`, `bastao.py`, `external_pair_api.py`, `scripts/omniroute-statusline.js:305-309` |
| `grupo-<gid>.md` (contrato do grupo) | os agentes, por `Edit`; `pair.py` funde e arquiva | agentes, `GET …/pair/contract` |
| `regras-<gid>.md`, `regras-draft-<hash>.md` (orquestração) | `orq_context.py`/`orq_md` | orquestrador, `pair.py` (funde/arquiva) |
| `external_pairs.json` (par externo, token do outro lado) | `external_pairs.py` | `links.rs`, `pair_hook.py`, `mcp_server.py` |
| `~/.hangar/pair-arquivo/` (contratos de grupo dissolvido) | `pair.py` | `orq_context.group_ended` |

`peers` lista os OUTROS membros: nome cru para membro local, `srv::nome` para membro de outra
máquina, onde `srv` é a **chave do `peers.json`** desta máquina. Legado `{"peer": x}` vira
`{"peers": [x]}`; sem `gid`, ele é derivado do conjunto de membros (`_gid_legado`, sha1 de 8 hex),
o mesmo cálculo em Python e Rust (`links.rs:111`).

### 1.2 Operações e chamadores

Rotas Python (o Rust repassa todas pelo `fallback`, `routes.rs:280`; área `pairing` em
`migration_status.rs:63-85`):

- `POST /api/sessions/{name}/pair` (`api.py:5101`): une os grupos de `name` e de cada peer
  (`pair.join_group`, `pair.py:162`), recusa tarefa diferente sem `replace_task` (409), entrega o
  protocolo só a quem estava solto, desfaz tudo se nenhum aviso chegou. Com peer remoto: só 1:1
  (`api.py:5115-5121`) por `_pair_cross_server` (`api.py:5172`), que grava o lado local e chama
  `POST {peer}/api/sessions/{x}/pair-remote` (`api.py:5229`).
- `DELETE /api/sessions/{name}/pair` (`api.py:5792`): `pair.leave` e `_avisar_saida`
  (`api.py:5707`), que chama `/unpair-remote` em cada peer remoto ou desfaz o par externo.
- `POST /api/sessions/{name}/group-message` (`api.py:5305`): `[grupo: <name>]` para cada peer,
  5 avisos/min por gid (429), recusa `[grupo:`/`[de:` reencaminhado.
- `GET /api/sessions/{name}/pair/contract` (`api.py:5692`): `{peers, path, content}`.
- `POST /api/pair/task-suggestion` (`api.py:5770`): resumo por LLM, sem estado de grupo.

Fora das rotas (`backend/app/`): `registry.py` limpa o grupo ao criar sessão com nome reusado
(`2433`, `2495`, `2547`), ao matar (`3078`, `3096`, `3118`), renomeia (`2972`, `3009`) e varre
mortos (`3132-3174`, laço de 2 s em `api._pair_sweep_loop`, ausência ≥ 5 s confirmada por tempo);
`api.kill_session` (`2708`) avisa remotos; `external_pair_api.py` (`246`, `252`, `268`) usa
`join_group`/`restore`/`leave`; `orq_context.associate` (`176`) segura `pair._LOCK`;
`pair.join_group` chama `orq_context.promote` (`pair.py:210`) e `leave`/`dissolve_lone_orq`
consultam `orq_runs.group_phase` (`pair.py:302,340`). MCP `pair`/`unpair`/`group`
(`mcp_server.py:156-180`) chamam as funções da API em processo. O vigia da orquestração usa HTTP
(`skills/orquestrar/scripts/vigia.sh`: 563 pareia, 604 lê a lista, 535 encerra sessão). `hangar-send --pair/--unpair/--group/--list`
usam HTTP no backend local (`scripts/hangar-send:216,378,646,679`).

### 1.3 Superfícies

- Lista: campos `pair_peers`, `pair_gid`, `pair_task`, `pair_external` (`hangar-api/src/session.rs:108-114`).
- PWA e app (`packages/core`): agrupam por `pair_gid`, cegos a servidor.
- Desktop nativo: cada máquina tem o seu bloco na barra lateral (`app.rs:4914-4930`); o agrupamento
  roda por máquina e, dentro dela, por seção (aguardando / projeto) (`app.rs:4877-4911`,
  `sidebar.rs:369-393`); `can_pair` recusa outra máquina (`grouping.rs:35`) e par remoto
  (`grouping.rs:41`); o painel do grupo é de uma máquina só (`group_sheet.rs:101,217-246`). O app
  identifica a máquina pelo endereço normalizado (`servers.rs:34`), não pelo `CP_SERVER_ID`; nada
  liga um ao outro.

### 1.4 Transporte entre máquinas

`peers.json` (do backend) mapeia chave → `base_url` + token de dono da outra máquina
(`peers.py:384-411`). A chave é escolhida por quem cadastra (`gravar_peer`, `_chave_livre`,
`peers.py:204,332`): por convenção é o `CP_SERVER_ID` da outra máquina, mas nada confere.
`peers.call` usa prazo de 8 s por leitura (16 s no total), corpo até 1 MiB, segue redirect, e
distingue "rejeitou" de "transporte incerto".
Confiança: máquina do `peers.json` = mesmo dono. **Par externo** é outra coisa: sessão de outra
pessoa, token de convidado `kind: "pair"`, só `POST /api/pair/message` e `DELETE /api/pair`,
recado assinado pelo registro (`[de fora: alias::sessao]`). Convidado de sessão compartilhada é
barrado das rotas de grupo por `share_gate._BLOCKED` (`share_gate.py:37`).

O Rust já tem: `reqwest` com rustls; ponte privada Python → Rust (`/__hangar_server/{list,…}`,
envelope `{"op","args"}`, `list/bridge.rs:788-810`); chamadas Rust → Python `/internal/*`; o caminho
de escrita do dono para sessão Claude, inclusive recado pelo socket nativo (`session_write/`,
`terminal_input.rs`); `owns` na saúde. O Rust **não** lê `peers.json` nem `CP_SERVER_ID`.

---

## 2. O que o Python faz bem (fica)

- Escrita atômica por arquivo e rollback por retrato quando uma escrita parcial falha.
- Tarefa diferente da existente exige `replace_task` (409).
- Protocolo só para quem entra; veterano não é acordado; grupo `orq` não recebe protocolo.
- Aviso do app como `[painel: grupo de trabalho]`, nunca `[de: …]`.
- Anti-tempestade de `--group` e recusa de reencaminhar `[grupo:`.
- Morte fora do app decidida por TEMPO de ausência (≥ 5 s), nunca com lista que falhou.
- Contrato de grupo dissolvido vai para o arquivo; fusão anexa o contrato.
- Grupo de 1 só existe no `orq` com execução `auto` viva.
- Par externo separado: dados, confiança e saída próprios.

## 3. Defeitos reais do Python (conferidos no código)

1. **Par entre máquinas nunca é um grupo só**: cada lado sorteia o seu `gid` (`pair.py:198`; o
   receptor chama `join_group` de novo em `api.py:5243`).
2. **Só 1:1 entre máquinas** (`api.py:5118`, `pair.py:185-190`), sem contrato (`api.py:5086-5087`).
3. **`--group` num par entre máquinas falha para o remoto**: confere `_session_exists("srv::x")`
   localmente e devolve "sessão não encontrada" (`api.py:5331-5334`).
4. **Saída do remoto derruba o grupo local inteiro**: `/unpair-remote` faz o membro local sair do
   próprio grupo (`api.py:5270`). Inofensivo só porque o par é 1:1.
5. **Sidecar órfão sem nova tentativa**: aviso de saída a máquina fora do ar só vai ao log
   (`api.py:5745-5748`); a varredura nem tenta (`registry.py:3173`).
6. **Renomear sessão de par entre máquinas apodrece o nome do outro lado** (`pair.py:353` é local).
7. **A lista não reemite quando só o grupo muda**: `pair_*` fica fora da assinatura no Python
   (`sse.py:345`) e no Rust (`list/sig.rs:68-88`), e as rotas não invalidam a lista.
8. **Varredura pede a lista ao Rust a cada 2 s** quando há grupo (`api.py:1066-1069`).
9. **Lock de processo**: `_LOCK` só protege o Python; `orq_context.associate` depende dele.
10. **Chave do `peers.json` ≠ `CP_SERVER_ID` do outro lado não é detectada**: o endereço
    `srv::nome` gravado lá não volta para cá.

---

## 4. Desenho

### 4.1 Dono das escritas

**Numa máquina, só o Rust grava a pasta `.hangar-pair` no modo `rust`/`pending`.** Os chamadores
Python (registry, par externo, MCP, rotas Python que chegam pela 8766/8768, `orq_context`) pedem ao
Rust pela ponte privada `/__hangar_server/groups`. Leitores Python continuam lendo o arquivo. Como
defesa, as primitivas de escrita do `pair.py` (`PairLink.set/clear`, `_merge_contract`,
`_arquivar_contratos`) recusam no modo `rust`/`pending`: um caminho esquecido falha alto em vez
de virar segundo escritor.

**Nenhuma rede nem entrega sob o lock de grupo.** Sob o lock só entram disco, a fila de saída e
`/internal/orq/*`. Entrega de aviso (`/input`) e chamadas a outras máquinas acontecem depois de
soltar o lock. Sem isso, um aviso esperando a porta de entrada de uma sessão que o Python fechou
para renomear travaria o renomear, que espera o lock.

No modo `python` (reserva do processo inteiro) o `pair.py` continua dono, com regras para grupo
entre máquinas (só existe na entrega 1b):
- entrar e renomear membro desse grupo: recusado com 409 `erro_grupo_entre_maquinas_sem_rust`
  (renomear é recusado **antes** de mexer no tmux ou no sidecar sem terminal);
- sair, matar, morrer, criar com nome reusado: o Python apaga o sidecar local e acrescenta o
  pedido de saída na fila de saída (`groups/outbox.json`, formato da Task do Rust), que o Rust envia
  quando voltar. O modo é único por processo, então isso não é segundo escritor.

Criar sessão cujo nome ainda tem sidecar e a limpeza falha: a criação é recusada
(`erro_grupo_limpeza_falhou`), para a sessão nova não nascer dentro do grupo da antiga.

### 4.2 Identidade

- **Membro = (servidor, nome)**, com servidor = `CP_SERVER_ID` da máquina dele.
- **Toda resposta das rotas de grupo diz o `server_id` de quem respondeu.** Quem chamou confere
  com a chave do `peers.json` que usou; diferente → 409 `erro_grupo_identificador_divergente`
  ("o `peers.json` chama a máquina de X, mas ela se identifica como Y"). Grupo entre máquinas
  exige `CP_SERVER_ID` nas máquinas envolvidas e chave igual a ele.
- **Grupo entre máquinas é identificado por (dona, gid)**, iguais em todas as máquinas.
- O nome continua sendo a identidade dentro da máquina, como hoje.

### 4.3 Dono do grupo

Cada grupo tem **uma máquina dona**, a única que grava o registro: membros (com servidor), tarefa,
versão e contrato.

- Grupo de uma máquina: dona = ela própria; o registro são os sidecars (formato de hoje).
- Grupo entre máquinas: dona = a que criou o grupo. Ela guarda `.hangar-pair/groups/<gid>.json`.
  As participantes guardam só os sidecars dos seus membros, com `fed`:

```json
{"peers": ["b", "lab::c"], "task": "ajuste do filtro de busca", "gid": "ab12cd34",
 "harness": {"a": "claude", "b": "codex", "lab::c": "claude"},
 "fed": {"owner": "casa", "local": true, "version": 7}}
```

```json
{"protocol": 1, "gid": "ab12cd34", "owner": "casa", "version": 7,
 "task": "ajuste do filtro de busca",
 "members": [{"server": "casa", "name": "a", "provider": "claude"},
             {"server": "casa", "name": "b", "provider": "codex"},
             {"server": "lab",  "name": "c", "provider": "claude"}]}
```

`fed` ausente = grupo de uma máquina ou par antigo 1:1. `fed.local` = a dona é esta máquina (o
hook decide por ele onde está o contrato). Leitores antigos ignoram `fed`.

Por que dono único: a regra de dissolução (grupo de 1), a tarefa, o contrato e o anti-tempestade
precisam de um lugar só; com dona, toda mudança tem ordem (a versão) e a réplica é o registro
inteiro.

### 4.4 Operações

Toda operação de grupo entre máquinas é aplicada pela dona sob o lock dela, sobe a versão e chega a
cada participante como o registro inteiro (`PUT /api/groups/{gid}`).

**Fila de saída** (`.hangar-pair/groups/outbox.json`; só o Rust lê, salvo o acréscimo de saída da
reserva Python):
- **Fila antes da rede.** A dona grava o registro e, no mesmo passo e sob o mesmo lock, o envio a
  cada participante; a participante grava os pedidos dela (saída, renomeação) do mesmo jeito.
- **Um trabalhador por máquina de destino, em ordem.** Todo envio passa por ele, inclusive o da
  entrada (que espera o resultado com prazo); sem canal direto paralelo, dois envios nunca chegam
  fora de ordem. Envio de registro é coalescido (só o mais novo por máquina e grupo); dissolução é
  um item próprio que substitui envio pendente do mesmo grupo.
- **Recusa definitiva sai da fila**: resposta 4xx com código ou máquina em versão antiga marca o
  item como feito e vai ao diário (`grupos.recusado`); só falha de rede tenta de novo (2 s, 5 s,
  30 s, 1 min, 2 min, 4 min, 5 min, 10 min, 30 min de teto), na subida e depois de qualquer
  resposta boa daquela máquina. Transições vão ao diário (`grupos.pendente`/`grupos.entregue`).
- Na subida, a dona reenvia todos os registros que possui; a participante pede à dona cada grupo
  em que tem membro.

**Regras da participante ao receber um registro** (fecham ressurreição e perda):
- aplica só versão maior que a sua; versão igual responde de novo o mesmo resultado, sem
  entregar protocolo outra vez;
- **só cria sidecar para quem foi aceito no `joining` daquele envio**; nos demais, só altera ou
  apaga sidecar que já existe com aquela (dona, gid);
- membro local que está no registro sem sidecar e não está em `joining` gera um pedido de saída
  para a dona (conserta perda e saída cuja resposta se perdeu);
- membro local que some do registro sem ter pedido para sair recebe o aviso de saída de hoje;
- renomeação pendente desta máquina é reaplicada por cima;
- dissolução apaga só sidecars cujo `gid` é o do grupo; dissolução por fusão (`merged_into`) não
  avisa ninguém.

Operações:

- **Entrar / criar** (`POST /api/sessions/{name}/pair`, corpo de hoje, `peers` pode ter
  `srv::nome`, N de cada lado). A máquina que recebe descobre o grupo de cada candidato (local:
  sidecar; remoto: `GET {srv}/api/sessions/{nome}/group`) e decide: nenhum tem grupo → cria como
  dona; um grupo → repassa à dona (`op: join`, levando o `gid` que viu de cada candidato; mudou
  desde então → 409); grupos da mesma dona → fusão na dona; donas diferentes → recusado (D2). A dona grava
  a versão nova e envia a cada participante quem entra ali (`joining`); a participante confere que
  a sessão existe na lista dela, não está em outro grupo e não é `orq` nem par externo, grava e
  entrega o protocolo. Recusado ou sem resposta sai na versão seguinte e volta no resultado; nenhum
  confirmado → 502 e nada fica.
- **Sair** (`DELETE …/pair`, kill, morte): a máquina do membro apaga o sidecar na hora, avisa a
  sessão e, se não é a dona, enfileira a saída para a dona. A dona remove e, se sobrar um membro no
  total, dissolve. Sair nunca depende da dona estar no ar.
- **Renomear**: mesma via (pedido à dona). Nada é recusado no modo `rust`.
- **Morte**: só a máquina do membro decide, pelo tempo de ausência na própria lista.
- **Aviso de grupo** (`--group`): a máquina do remetente pede à dona (`op: message`); a dona aplica o
  anti-tempestade e entrega a todos (locais direto; remotos por `POST /api/groups/{gid}/deliver`).
  Quem não foi alcançado volta em `pulados`. Dona fora → 503 `erro_grupo_dono_indisponivel`.
- **Contrato**: arquivo na dona (`grupo-<gid>.md`). Membro na dona edita pelo caminho. Membro fora
  usa `hangar-send --contrato` (ler) e `--contrato --gravar <mtime>` (stdin); a máquina dele
  repassa à dona. Gravação confere `mtime` (409 `erro_contrato_mudou`); criação é exclusiva.
  Dona fora = contrato indisponível, com frase própria (sem réplica: cópia velha de texto vivo
  engana).

### 4.5 Rotas

Públicas (dono do Hangar; corpo de hoje vale igual):

| Rota | Muda |
|---|---|
| `POST /api/sessions/{name}/pair` | `peers` aceita N remotos; resposta igual |
| `DELETE /api/sessions/{name}/pair` | saída chega a todas as máquinas; par externo desfeito como hoje |
| `POST /api/sessions/{name}/group-message` | alcança todas as máquinas; `pulados` passa a ter conteúdo |
| `GET /api/sessions/{name}/pair/contract` | repassa à dona; ganha `owner` e `mtime`; `path` vazio quando a dona é outra |
| `PUT /api/sessions/{name}/pair/contract` | nova: `{content, mtime}` → `{mtime}` ou 409 |
| `GET /api/sessions/{name}/group` | nova: `{server_id, gid, owner, version, task, orq, members, sync}` ou 404 `erro_sessao_sem_grupo`; barrada ao convidado (`share_gate._BLOCKED`) |
| `POST /api/pair/task-suggestion` | fica no Python |

Entre máquinas próprias (token de dono do `peers.json`; corpo estrito, `protocol: 1`):
`GET|PUT /api/groups/{gid}`, `POST /api/groups/{gid}/ops`, `POST /api/groups/{gid}/deliver`,
`GET|PUT /api/groups/{gid}/contract`. Só no Rust (o Python não as tem, então 8766/8768 não as
alcançam). Máquina antiga: 404 sem código, 405 ou 422 numa rota `/api/groups/*` viram
`erro_grupo_peer_versao_antiga`.

### 4.6 Par antigo 1:1 entre máquinas

**Fica como está, sem conversão.** Sidecar sem `fed` com um peer `srv::x` continua 1:1: a saída
chama o `/unpair-remote` do outro lado, e o `/unpair-remote` e o `/pair-remote` que chegam de
máquina antiga continuam atendidos (portados na entrega 1a com paridade). Pedido novo de
pareamento entre máquinas sempre nasce grupo `fed` (1b); outra máquina em versão antiga →
`erro_grupo_peer_versao_antiga`. Converter exigiria as duas máquinas concordarem sobre identidade
e dona sem conversar, o que a chave livre do `peers.json` não garante; o par antigo some sozinho
quando alguém o desfaz.

### 4.7 Orquestração

Grupo `orq` é sempre de uma máquina: remoto em grupo `orq` é recusado
(`erro_grupo_orq_entre_maquinas`). O Rust grava os sidecars `orq`, pede ao Python
`/internal/orq/promote` quando um grupo `orq` nasce (sob o lock, desfazendo os sidecars se falhar)
e `/internal/orq/group-phase` antes de dissolver grupo `orq` (falha = `unknown` = mantém). A
associação do time (`orq_context.associate`) passa a ser a operação de ponte `group.orq_associate`:
o Rust toma o lock e chama `/internal/orq/associate`, que roda o corpo de hoje sem o `pair._LOCK`.
Os arquivos `regras-*` continuam do Python; o Rust só funde e arquiva.

### 4.8 Par externo

Continua 1:1, separado e no Python (convite, token, recado de fora, confiança). O sidecar
`<local>.json` passa a ser gravado pelo Rust a pedido do Python (`group.external_link`,
`group.external_unlink`). Toda saída feita no Rust (`DELETE …/pair`, varredura) de sessão com par
externo chama `/internal/external-pairs/end`, que desfaz o lado de fora como hoje
(`registry._encerrar_pares_externos`). Sessão em par externo não entra em grupo e vice-versa.

### 4.9 Texto aos agentes

Fonte única continua `pair_texto.py`. O Rust pede o texto ao Python (`/internal/pair/text`) quando
entrega o protocolo, fora do lock. Mudanças (1b): linha do contrato sempre presente (caminho se a
dona é esta máquina, `hangar-send --contrato` se não é); membro de outra máquina endereçado como
`srv::nome`; aviso de grupo alcança todas as máquinas. O hook escolhe pela `fed.local`.

### 4.10 Lista e telas

- Linha ganha `pair_owner` (id da dona; só em grupo `fed`). `pair_peers` continua relativo à máquina
  da linha. `pair_*` entra na assinatura da lista (Python e Rust) e toda escrita de grupo invalida
  a lista.
- PWA e app: nada novo; como agrupam por `pair_gid`, um grupo `fed` aparece junto lá também. Frases
  dos códigos novos em `messages/*.json` e no mapa do core.
- Desktop nativo:
  - pergunta o `server_id` de cada máquina (`GET /api/config`, `somente_leitura.server_id`);
  - modelo global de grupos com chave (dona, gid) ou (máquina, gid); par externo (`pair_external`)
    nunca vira membro; par antigo 1:1 junta as duas metades quando cada lado aponta para o outro;
  - **seção "Grupos" no topo da barra lateral (D1, decidido)**: cada grupo é um bloco só, com todos os
    membros e o selo da máquina em cada linha; membros saem dos blocos por máquina e das seções
    aguardando/projeto; o cabeçalho diz quantos aguardam;
  - estados de membro: presente; **máquina fora do ar** (última linha boa, esmaecida, "máquina
    indisponível"); **máquina sem conexão neste app** (linha sem sessão); **carregando**. Membro
    que a própria máquina, no ar, diz que saiu não aparece. Nada é inventado como sessão viva e
    nenhum vínculo é apagado pela tela;
  - arrastar sessão de uma máquina sobre sessão de outra pareia; o painel do grupo lista membros de
    todas as máquinas, lê o contrato da dona e a conversa de cada membro na máquina dele.

### 4.11 Windows e reserva

O Rust já roda no Windows; a troca de arquivo usa o helper com novas tentativas de
`hangar-workspace`. Hook e `hangar-send` não mudam de mecanismo. Não conferido nesta análise: o
`hangar-send` no Windows com o verbo novo e a volta à reserva Python no Windows com grupo `fed`
vivo; ambos estão no roteiro de verificação manual.

---

## 5. Exclusões da proposta de 21/09, reavaliadas

| Exclusão de 21/09 | Agora | Por quê |
|---|---|---|
| Fundir grupos que já existem em máquinas diferentes | Mesma dona: sim. Donas diferentes: recusado (D2) | Fusão da mesma dona é escrita de um lugar só; donas diferentes exigem duas. |
| Aviso de grupo atravessando máquina | Entra | Sem ele um 2+2 avisa metade do time e o protocolo mente; com dona, o anti-tempestade fica num lugar. |
| Migração do par 1:1 vivo | Continua fora | Converter sem conversar depende de a chave do `peers.json` ser o id do outro lado, o que não é garantido; o par antigo segue funcionando como 1:1. |
| Varredura de morto atravessando máquina | Continua fora | Só a máquina do membro sabe se ele morreu; concluir de longe apagaria vínculo por queda de rede. |
| Editar o contrato pela interface | Continua fora | Não foi pedido; as telas leem. |
| Renomear sessão de grupo entre máquinas | Passa a valer (pela dona) | É a mesma via da saída; recusar travava uma ação comum. |

## 6. Decisões

Tomadas neste desenho (contestáveis):

- Dona única por grupo; réplica = registro inteiro com versão; fila em disco antes da rede.
- Reserva Python não entra nem renomeia em grupo `fed`; saída funciona e fica na fila.
- Contrato sem réplica; dona fora = contrato indisponível. Aviso de grupo com a dona fora = erro.
- Par antigo 1:1 não é convertido.
- **Três entregas, nesta ordem**: **1a** migração com paridade (o Rust passa a gravar tudo o que o
  Python grava hoje, inclusive o par 1:1 entre máquinas; goldens contra o Python; pode juntar
  sozinha); **1b** federação (N em M, dona, fila, aviso e contrato entre máquinas, texto aos
  agentes); **2** desktop nativo. A 1a porta o iniciador 1:1, que a 1b troca; o receptor e o
  `/unpair-remote` ficam de qualquer forma, pela máquina antiga. Em troca, a parte de maior risco
  (federação) entra sobre uma base já conferida no uso real.

Decididas pelo dono:

- **D1 — onde o grupo aparece no nativo: seção "Grupos" no topo com todos os grupos**
  (08/10/2026), de uma máquina ou de várias. Os membros saem dos blocos por máquina e das seções
  aguardando/projeto; o grupo nunca se parte. Deixar os grupos de uma máquina no bloco dela foi
  descartado: os grupos ficariam em dois lugares.
- **D2 — fusão de grupos com donas diferentes: recusar** (08/10/2026). A recusa diz para desfazer
  um dos grupos e juntar as sessões de novo (`erro_grupo_fusao_entre_donos`). Mover membro a membro
  foi descartado: não é atômico e o contrato do grupo menor iria para o arquivo.
