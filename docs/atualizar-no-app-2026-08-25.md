# Botão "Atualizar" no app — material para planejar

Levantamento feito em 25/08/2026, com o repo em `f4013343`. Tudo que afirma coisa sobre o código
atual vem com arquivo:linha conferido nessa data. É material de discussão: traz opções e o que
custa cada uma, não um plano fechado.

O pedido: hoje, quando sai versão nova, alguém precisa avisar cada pessoa ("atualiza aí") e, quando
a atualização exige passo extra, dizer qual comando rodar. O botão faria isso sozinho: mostra o que
mudou, o que precisa ser feito, e executa.

## O que já existe (e é mais do que parece)

Metade do motor está pronta. Antes de desenhar qualquer coisa nova, vale saber o que já roda:

| Peça | Onde | O que faz |
|---|---|---|
| Atualização completa não interativa | `scripts/deploy.sh` | fetch, `merge --ff-only` (aborta se divergiu), `npm ci` só se o lock mudou, backup do `dist` + build + **restaura o dist antigo se o build falhar**, `uv sync` só se o lock mudou, restart só depois do build ok |
| Reaplicar o que o pull não atualiza | `install.sh --update` / `install.ps1 -Update` | units systemd com caminho cravado, bloco de protocolo no `~/.claude/CLAUDE.md`, deps, build do front |
| Disparo automático hoje | `scripts/post-merge.hook` | roda o installer no modo `--update` depois de todo `git pull`/merge — e recusa rodar dentro de worktree |
| Migração de dados versão-a-versão | `backend/app/migracao_sidecars.py`, chamado em `backend/app/main.py:99` | renomeia `.claude-pocket-*` → `.hangar-*` na **subida do backend**, deixando link no caminho antigo |
| Estado do repo por API | `backend/app/git_ops.py` | `git_summary` (branch, sujo, ahead/behind), `git_action` (pull/fetch), `push`, `reset_to`, `switch_branch`, `git_log` — já expostos em rotas e já com tela Git no app |
| Versão que a tela mostra | `frontend/vite.config.ts:13` → `__HANGAR_VERSION__`, usada em `SobreSettings.svelte:10` | `git describe --tags --always --dirty` no momento do **build** |
| Versão no diário de uso | `backend/app/diag.py:226` | o mesmo `git describe`, rodado no backend, no cabeçalho do download |

Duas conclusões que isso já resolve:

**O `deploy.sh` é o botão sem interface.** Ele já é idempotente, já é ff-only e já tem o rollback
do front. O trabalho não é escrever um motor de atualização — é dar cara a esse, cobrir os casos que
ele não cobre (Windows, repo sujo, passos por versão) e mostrar o resultado.

**O rename que obrigou o hard reset foi resolvido de outro jeito, e o jeito é bom.** O
`migracao_sidecars.py` roda na *subida do backend*, não no installer, com a justificativa escrita no
`CLAUDE.md`: atualizar é `git pull` + reiniciar o serviço, e rodar `install-*.sh` não é garantido.
Ou seja: já existe um lugar onde passo de migração de **dados** roda sozinho, e a regra é que ele
nunca funde nada — para com aviso quando o destino já existe. O botão precisa cobrir a outra
categoria: passo que mexe no **repo/ambiente** (dependência nova, unit nova, arquivo que precisa
sair da frente).

---

## 1. Como saber em que estado a máquina está

Nenhum desses estados pode ser assumido. O que dá pra detectar, com o quê, e o que fazer:

| Estado | Como detectar | Proposta |
|---|---|---|
| **Árvore suja** | `git status --porcelain` (já em `git_ops.git_summary`) — separar rastreado modificado de não-rastreado | Rastreado modificado: **para e mostra a lista**. Não-rastreado: segue (não atrapalha o ff), mas menciona |
| **Branch ≠ main** | `git branch --show-current` (`git_ops.branch_of`) | Para. Oferece "voltar pra main" como ação explícita, nunca automática |
| **Commit local** (ahead) | `git rev-list --count @{u}..HEAD` | Para. É trabalho da pessoa; um ff-only falharia de qualquer jeito |
| **Divergiu** (ahead+behind) | ahead>0 e behind>0 | Para. É exatamente o caso do hard reset — só sai daqui com ordem explícita |
| **Repo em caminho diferente** | O backend já sabe: `Path(__file__).resolve().parents[2]` (`diag.py:226`) | Não é problema — quem atualiza é o backend, no próprio checkout |
| **Serviço systemd ou não** | `systemctl --user list-unit-files hangar-backend.service` (exit code, não stdout — precedente no `deploy.sh`) | Três topologias válidas: systemd, Windows (tarefa + `.vbs`), e nada (rodando na mão). A terceira **não pode** ser tratada como erro |
| **Front: dist ou vite** | A unit `hangar-frontend.service` existe? (`npm run preview` = dist; dev server = HMR). E há instalação onde o backend serve a UI e a unit não existe | `deploy.sh` já resolve isso com o teste de unit. O build é obrigatório só quando alguém serve `dist` |
| **Node/uv de versão diferente** | Rodar `node -v` / `uv --version` **antes** de mexer em qualquer coisa | Falta de dependência = **recusa antes de começar**, com o nome do que falta. Precedente: `install.sh:90` — "`--update` não instala dependência" |
| **Windows** | `os.name` no backend | Sem systemd: tarefas agendadas + `.vbs` + log em `%LOCALAPPDATA%\hangar\`. O `install.ps1:1242` já registra que um `-Update` chegou a dizer "ok" com o processo **antigo** ainda no ar |

### A armadilha nº 1: "a versão instalada" tem três respostas

Isso muda o desenho e vale decidir antes de qualquer código:

1. **O commit do repo** (`git rev-parse HEAD`) — o que o disco tem.
2. **O commit do backend em execução** — o processo subiu antes do pull e segue rodando código
   antigo até reiniciar.
3. **O bundle que o navegador da pessoa está mostrando** — é um PWA com service worker
   (`precache 73 entries` no build de hoje). O `__HANGAR_VERSION__` é gravado **no build**; a tela
   pode ser de dias atrás mesmo com repo e backend novos.

Um botão que compara só (1) com o `origin/main` dirá "tudo atualizado" para alguém olhando uma tela
velha. **A tela precisa mostrar as três**, e o passo final da atualização tem que forçar o front a
recarregar — senão a pessoa atualiza, vê a mesma tela e conclui que não funcionou.

As três já existem, e nenhuma precisa de código novo pra ser lida:

- **repo**: `git describe --tags --always --dirty`.
- **backend em execução**: `diag.VERSAO_EM_EXECUCAO` (`backend/app/diag.py:94`), resolvida **na
  importação do módulo** e nunca mais. Isso é de `f4013343`, e o motivo é exatamente esta seção:
  até esse commit o valor era lido na hora do download, dentro do checkout — ou seja, durante a
  janela entre o `git pull` e o restart o backend afirmava rodar um código que ninguém tinha
  carregado. É a janela em que o botão de atualizar vive.
- **bundle no navegador**: `__HANGAR_VERSION__`, gravado no build (`frontend/vite.config.ts:13`) e
  já enviado ao diário no evento `app.abriu`, junto com sistema, navegador, desktop/celular e
  resolução.

`/api/config` (`api.py:2741`) hoje **não** devolve o commit do backend. É o primeiro dado a
acrescentar, e o valor já está pronto em `diag.VERSAO_EM_EXECUCAO` — falta decidir o nome do campo
e onde pendurar.

---

## 2. Como atualizar sem quebrar

O requisito duro é: pela metade é pior que não ter. Três coisas dão isso, e duas já existem.

### Ordem que não deixa estado misto

A do `deploy.sh`, que é a certa e vale copiar: **tudo que pode falhar acontece antes de qualquer
coisa irreversível.** Verificação → ff-only → deps → build (com backup) → só então restart. Se o
build falha, o `dist` antigo volta e **o restart não acontece**: a pessoa fica na versão anterior,
inteira.

O que falta ali: o backend novo já está no disco quando o restart roda. Se ele não sobe (erro de
import, dependência nova), fica-se sem backend. Duas saídas:

- **Health check depois do restart** — sobe, espera o `/` responder 200 (~3s), e se não responder
  faz `git reset --hard` para o commit anterior + rebuild + restart. É rollback de verdade, mas o
  `reset --hard` é justamente o comando que a regra do repo proíbe sem ordem explícita. Só que aqui
  ele volta para um commit que **acabou de sair da própria máquina**, sem trabalho da pessoa
  no meio — o pré-voo garantiu isso.
- **Só avisar** — deixa quebrado e mostra o log com o comando pra voltar. Mais simples, mais honesto,
  e transfere o problema pra pessoa que o botão existia pra poupar.

Recomendação: health check com rollback automático, **porque o pré-voo já provou que não há nada
para perder**. Se o repo estava sujo ou divergido, o botão nem chegou a essa etapa.

### Idempotente = executar duas vezes não muda o resultado

Serve de critério pra aceitar ou recusar um passo declarado. `git merge --ff-only` já é (segunda vez
= "already up to date"). `npm ci`, `uv sync` e o `install.sh --update` também. Um passo que só
funciona uma vez (mover arquivo, apagar pasta) tem que ser escrito com guarda — `migracao_sidecars`
é o modelo: destino já existe, para com aviso, não funde.

### Saber se deu certo

Cada passo declara sua própria prova, e a prova roda depois. Sem isso "sucesso" quer dizer só "o
comando saiu com 0", que é o que produziu o `-Update` dizendo ok com processo antigo no ar
(`install.ps1:1242`). Prova = comando cujo exit code responde: `curl` no backend, `test -f` no
arquivo que deveria existir, `systemctl is-active`.

A prova mais forte é gratuita e já existe: **comparar as três versões depois**. Se o repo, o
`diag.VERSAO_EM_EXECUCAO` e o `__HANGAR_VERSION__` da tela batem, a atualização chegou nas três
pontas — inclusive na do navegador, que é a única que a pessoa vê.

### O rastro, se der errado, já está instrumentado

O diário de uso grava toda ação (POST/PUT/PATCH/DELETE) com sucesso e falha, e o ciclo de vida do
SSE — `sse.abrir`, `sse.caiu` com o motivo, `sse.mudo` — com `X-Hangar-Req` ligando a linha da tela
à do servidor. Se a atualização derrubar alguma coisa, o rastro fica no arquivo do dia sem
instrumentar nada novo: o `sse.caiu` do restart e o `sse.abrir` da volta delimitam a janela, e o
cabeçalho do download já diz de qual versão veio. O botão não precisa de log próprio; precisa, no
máximo, registrar seus próprios passos como eventos ali.

### O backup do dist ainda cabe (medido)

`build.sourcemap: true` entrou no último lote, e o `deploy.sh` faz `cp -a dist dist.bak` antes do
build. Medido nesta máquina em 25/08/2026: `dist` tem **19 MB**, dos quais **13,1 MB são os 59
arquivos `.map`** — o restante são 5,1 MB. O `cp -a` do diretório inteiro levou **0,014s**, e o pico
de espaço durante o build é ~38 MB. Continua irrelevante; não vale complicar o passo com cópia
seletiva.

---

## 3. Onde declarar os passos por versão

Um arquivo versionado no repo, como você descreveu. As decisões abertas são o formato e a chave.

### A chave: commit, não número de versão

O repo tem 5 tags e 124 commits desde a última (`v0.1.2-124-g490a71c5`) — versão semântica não é
mantida aqui, e criar essa disciplina agora é trabalho que o botão não precisa. **Como a atualização
é `git pull`, o git já sabe responder "o que entrou entre onde eu estava e onde vou parar":**
`git log OLD..NEW`. Um passo declarado num commit é um passo que entrou naquele intervalo.

### Duas formas de guardar "o que já rodou aqui"

**(a) Comparar intervalo de commits.** Guarda o commit anterior, roda os passos que entraram em
`OLD..NEW`. Simples, e o git faz a conta. Fura em dois casos reais: instalação nova (não há OLD, e
rodar a história inteira de passos é errado) e pessoa que fez reset/reclonou.

**(b) Registro de passos aplicados** — o padrão de migração de banco (Django, Rails, Flyway). Cada
passo tem um id fixo; um sidecar local (`~/.claude/.hangar-update/aplicados.json`) lista os ids já
executados; o botão roda o conjunto que falta. Não depende de onde a máquina estava, sobrevive a
reset, e uma instalação nova nasce com todos marcados como aplicados (o instalador já fez tudo).

Recomendação: **(b)**, com (a) só para *mostrar* o changelog na tela. O registro custa um arquivo e
elimina a classe inteira de "não sei de onde essa máquina veio". Mesma família do que
`migracao_sidecars` já faz, só que explícito.

E se for (b), o lugar de rodar os passos provavelmente é o **mesmo do `migracao_sidecars`: a subida
do backend** (`main.py:99`), não o installer. O motivo já está escrito lá e vale igual aqui —
atualizar neste projeto é `git pull` + reiniciar o serviço, e ninguém garante que um `install-*.sh`
rodou. Isso também dá de graça o caso da pessoa que atualizou pela linha de comando sem tocar no
botão: os passos que faltavam rodam no restart seguinte, de qualquer jeito. O botão vira a *cara* do
processo, não o único caminho por onde ele passa.

### Formato

Um arquivo por passo em `docs/atualizacoes/` (ou `updates/`), com frontmatter e corpo em markdown —
o app já renderiza markdown em tudo (`lib/markdown.ts`, regra do `CLAUDE.md`). Um arquivo por passo,
e não um `.json` único, porque dois commits que tocam o mesmo `.json` conflitam e o de um só toca no
próprio arquivo. Esboço:

```markdown
---
id: 2026-08-25-rename-hangar
titulo: Rename claude-pocket → hangar
desde: b2e86c91        # opcional; só quem passou por aqui precisa
automatico: true       # false = só mostra o texto, a pessoa executa
comando: ./install.sh --update
prova: test -x ~/.local/bin/hangar-send
risco: baixo           # baixo | pede-confirmacao | destrutivo
---

O que mudou, em uma frase para quem usa. Se `risco` for maior que baixo, o que
exatamente vai acontecer com o que está no disco.
```

E o changelog que a tela mostra: **os títulos dos commits do intervalo**, não um arquivo à parte.
As mensagens de commit deste repo já são descritivas (`fix(front): faixa Agora revalida sozinha…`).
Manter um `CHANGELOG.md` à mão é uma segunda cópia que envelhece; se um commit merecer texto de
release, ele ganha um arquivo de passo com `automatico: false` e só texto.

---

## 4. A linha entre executar sozinho e pedir confirmação

Proponho três faixas, e a regra é o que acontece com **trabalho da pessoa**:

**Executa sozinho** (repo limpo, na main, sem commit local — o pré-voo já verificou):
`git merge --ff-only`, `npm ci`, `uv sync`, build do front, `install.sh --update`, restart do
serviço, passo declarado com `risco: baixo`. Nada aqui apaga nada; o pior caso é o rollback já
descrito.

**Pede confirmação** — mostra na tela exatamente o que vai acontecer, com a lista, e só age no
segundo toque: passo com `risco: pede-confirmacao`, trocar de branch de volta pra main, `git stash`
do que está sujo, e **qualquer restart quando há sessão trabalhando** (item 5).

**Nunca sozinho, mesmo com confirmação genérica**: `git reset --hard`, apagar arquivo não-rastreado,
descartar commit local, `git clean`. A regra do repo é não destruir sem ordem explícita, e "ordem
explícita" aqui quer dizer que a tela nomeia o que se perde ("3 arquivos modificados, 1 commit
local") e a pessoa confirma **isso**, não um "atualizar" genérico. O botão pode oferecer o caminho
não-destrutivo (branch de resgate com o trabalho, depois o reset) — assim nada se perde de verdade.

A exceção que discutimos acima é o **rollback automático**: `reset --hard` para o commit que a
própria máquina tinha minutos antes, sem nada da pessoa no meio, com a alternativa sendo ficar sem
backend. Vale marcar isso explicitamente como exceção declarada, não como brecha na regra.

---

## 5. Atualizar com sessão viva

O que foi medido hoje nesta máquina, reiniciando backend e frontend com sessões abertas:

- **A sessão do agente não morre.** Ela roda em `tmux`, fora do backend — `hangar` e `hangar-2`
  seguiram vivas depois do restart. Esta conversa continuou.
- **O que cai**: as conexões SSE (o front reconecta sozinho; há watchdog de 25s), o WebSocket do
  terminal, e os app-servers do Codex, que são subprocessos do backend
  (`adapters/codex/appserver.py:51`) — sessão Codex precisa reconectar.
- **O que sobrevive**: fila durável, marcadores de estado, sidecars — tudo em disco.

### App-server órfão do Codex: no Linux o systemd resolve, no Windows não

A armadilha conhecida é que o app-server do Codex sobrevive ao `kill-session` da sessão tmux e fica
escutando em loopback, e limpar exige matar por pid. Vale saber se ela aparece no restart, porque é
uma das poucas coisas que o botão deixaria para trás. Conferido em 25/08/2026:

- O app-server nasce como subprocesso **direto** do backend (`create_subprocess_exec`), sem
  `systemd-run --scope` — ao contrário das sessões tmux, que ganham escopo próprio justamente para
  não herdar o cgroup do backend.
- `hangar-backend.service` está com `KillMode=control-group` (o padrão) e `TimeoutStopUSec=10s`.

Ou seja: no Linux, `systemctl --user restart` mata o cgroup inteiro e leva os app-servers junto —
não sobra órfão, e não há passo a acrescentar. **O caso que fica em aberto é o Windows**, onde não há
cgroup nem systemd: o restart é matar o processo, e os filhos não morrem por tabela. Ali o passo de
restart provavelmente precisa matar a árvore, e é o mesmo lugar onde `install.ps1:1242` já registra
um `-Update` que dizia "ok" com o processo antigo no ar. Vale confirmar na máquina Windows antes de
escrever o passo — nenhuma das duas coisas foi medida lá.

Então o risco real não é perder trabalho, é **o restart cair no meio de um turno** e a pessoa ver a
tela congelar sem entender por quê, ou o front recarregar em cima de um texto que ela estava
digitando.

Opções:

- **Perguntar quando há sessão `working`** — o backend já sabe o estado de todas
  (`registry.list_with_state`). "2 sessões trabalhando agora. Atualizar mesmo assim?" Simples, e
  informação boa.
- **Esperar todas ficarem `idle`** (com teto de tempo) — mais gentil, mas pode nunca acontecer, e
  fila de espera é mais código.
- **Avisar as sessões antes** — `hangar-send` já entrega recado às sessões vivas. Custa quase nada e
  a sessão pode se preparar.

Recomendação: perguntar quando há alguém `working`, e avisar por `hangar-send` que o backend vai
reiniciar. Esperar `idle` sozinho é a versão elaborada, e vale só se a simples incomodar.

Vale lembrar: se a pessoa está no **celular**, o restart significa a tela reconectando. O front
precisa mostrar isso como "atualizando…", não como erro de conexão — hoje ele mostraria
"desconectado", que é a mesma coisa que a tela mostra quando o servidor caiu de verdade.

---

## O que precisa de decisão sua

1. **Registro de passos aplicados (sidecar) ou intervalo de commits?** Recomendo o sidecar; muda o
   tamanho do trabalho.
2. **Rollback automático com `reset --hard` para o commit anterior, ou só avisar e parar?**
3. **Escopo agora**: só esta máquina/uma máquina por vez, ou o botão atualiza também os peers
   (`peers.json`) a partir de uma tela? O app é multi-servidor; a segunda opção é um trabalho
   bem maior.
4. **Onde o botão mora**: barra do app, ou linha em Configurações (que é onde o Diário de uso
   acabou de nascer)? O `CLAUDE.md` diz que config abre em modal, não em painel docado.

Fontes consultadas sobre o padrão geral (backup antes, versão fixada, migração com runbook e
rollback, blue-green por diretório de release):
[oninitiative](https://www.oninitiative.com/blog/technology/self-hosted-app-update-rollback-keep-ai-apps-stable-stop-downtime-cascades/) ·
[tech-champion](https://tech-champion.com/database/safe-database-migrations-for-self-hosted-applications-a-practical-release-engineering-guide/) ·
[dev.to/argo-cd](https://dev.to/jamesli/argo-cd-application-updates-rollbacks-gitops-driven-version-control-in-practice-115g)
