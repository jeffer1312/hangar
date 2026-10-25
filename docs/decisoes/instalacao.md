# Instalação, atualização e serviços

Decisões medidas, com data e número. As regras vigentes ficam na seção abaixo (o `CLAUDE.md`
só aponta para cá); a medição que sustenta cada uma mora na entrada de mesmo assunto.

## Regras vigentes

- **Quem serve a interface é o BACKEND; o `frontend/dist` chega pronto do CI.** Gate que pergunta
  "o front mudou?" tem que conhecer TODAS as entradas de onde o front é compilado (`frontend`,
  `packages`, `messages`, `project.inlang` e o `package.json`/`package-lock.json` da raiz) — senão
  serve tela velha ou apaga edição local.
- **Instalador com portão de prova por etapa**: essencial que falha para na hora; extra opcional
  que falha entra na lista e o fim nunca diz "Pronto". Nada é dado como feito sem prova.
- **O backend escuta em `0.0.0.0` por padrão**, gravado pelo instalador e pelo `--update`/
  `-Update` quando o `.env` está sem a chave ou em loopback; IP escolhido à mão fica. É o que
  deixa os clientes usarem a rede local antes do Tailscale (`baseOf`, em
  [plataforma.md](plataforma.md#rede-local-antes-do-tailscale-baseurl-é-identidade-a-rota-é-baseof)).
  Nunca `auto`: tira o loopback de que o `tailscale serve` depende. Firewall do Windows no
  `-Update` só sem UAC (já admin); senão vira pendência com o comando.
- **Instalador guiado: duas perguntas, o resto é padrão.** Sem terminal, tudo é NÃO — exceto no
  `--app`/`-App` (assistente do app nativo), que responde pelas telas: `ask` e `ask_senha` sim,
  `ask_extra` não, Tailscale pela opção, nada espera teclado. O log nunca carrega o token, e no
  `--app` nem a saída: a senha chega por `HANGAR_TOKEN`. O portão do 1/8 do `install.ps1` só barra
  pendência sem código; as com código são extras e vão ao portão do fim. A senha do celular recusa
  ASCII não visível (acento), `#`, `$`, aspas, `\` e espaço nas pontas (o `.env` é lido pelo python-dotenv; o app nativo monta o cabeçalho só com ASCII visível). Ver
  [a entrada](#modo---app-o-assistente-responde-pelas-telas).
- **O instalador exige UM agente de código, não o Claude Code.** Usa os que já existem; nenhum →
  instala o Claude Code (o padrão). `--agentes=`/`-Agentes` e o `--avancado` escolhem; o comando
  dos que não são o Claude sai de `app/harness_commands.py`, a mesma tabela do painel de
  Harnesses. Ver [a entrada](#o-instalador-exige-um-agente-de-código-não-o-claude-code).
- **Atualizar pelo app faz tudo sozinho, mas nada é irreversível**: resgate antes de qualquer
  passo destrutivo, com a ref conferida. Passo só entra no registro depois da prova passar, e o
  registro é do que JÁ RODOU aqui — não do intervalo de commits.
- **A volta da atualização devolve o binário do Rust junto com o código.** Antes de gravar o
  binário novo, a atualização guarda o anterior em `~/.hangar/bin/<nome>.prev`; a volta o
  devolve antes de reiniciar. Sem isso, código antigo com binário novo é contrato diferente e o
  Python atende sozinho. A cópia de outra rodada é descartada antes de instalar e ao terminar bem.
- **Falha depois do código novo volta tudo, mesmo antes do reinício.** Passo ou instalação que
  falha com o checkout já trocado passa pela mesma volta do reinício (código, binário, tela e
  reinício): o próximo reinício, de qualquer origem, subiria versões que não se entendem.
- **Dentro do Atualizar, só ele baixa o binário do Rust.** Ele marca o ambiente
  (`HANGAR_UPDATE_OWNS_RUST=1`) e o `python -m app.rust_release` dos instaladores chamados pelos
  passos não baixa; senão vinha o binário do topo da release, sem a cópia que a volta devolve.
  Passo que falha mostra as linhas `##HANGAR-FALHA##` do instalador, não as últimas linhas.
- **O Atualizar segue a main, ou a branch de `CP_UPDATE_BRANCH`.** Só ela e a branch em que a
  própria atualização pôs o checkout (`<config>/.hangar-update/branch`) escapam da recusa de
  branch de trabalho; esvaziar o campo volta para a main com o mesmo resgate. Branch ausente no
  origin falha antes de tocar no disco, e fora da main a tela é compilada aqui (o CI só publica o
  dist da main) e o auto-update fica parado. Passo de `docs/atualizacoes/` aplicado na branch de
  teste continua no registro ao voltar pra main; e, esvaziado o campo, o auto-update não tira o
  checkout da branch de teste: a volta é pelo botão.
- **O Atualizar só avança até o commit cujo binário do Rust foi publicado para este sistema.**
  O binário é o de `platforms.<sistema>` no `server-latest.json` da release da branch (sem a
  chave, o `commit` do topo); o contrato é o `protocol` dele ou o `RUST_SERVER_PROTOCOL` do commit. O checkout vai
  do commit desse binário até o mais novo do primeiro pai com conteúdo idêntico ao publicado nos
  `paths` do push no `.github/workflows/server.yml` de lá, nunca recua, e o download usa o manifesto lido na
  escolha; sem ler a lista, para no commit do binário. Só binário de commit fora de `origin/<branch>`
  cai no commit mais novo com o mesmo número. A tela conta as mudanças até esse commit e também oferece atualizar quando o binário
  instalado não é o publicado (código em dia, binário que saiu depois). Atrás do topo, a tela vem
  da release `dist-commits` pela chave das fontes (regra abaixo) e só é compilada aqui sem ela (também
  no Reiniciar); a atualização até o último binário publicado é completa e
  normal: build pendente no CI não vira aviso nem "incompleta" em nenhum cliente, só o registro
  `atualizacao.limitada_ao_binario` no diário; o servidor ainda zera `waiting_for_server_build` para
  cliente antigo. Quando o botão de cima do nativo aparece segue o contrato `CLIENT_API` (regra
  abaixo). Checkout já à frente com binário de outro contrato continua
  avisando. Evidência em [Build pendente não é aviso](#build-pendente-no-ci-não-é-aviso-19102026). Sem Rust (`CP_RUST_SERVER=0`) ou sem release própria de branch fora da main (404): topo,
  como antes, calado. Sem como conferir (rede, git, 404 na release da main, sem build deste
  sistema no manifesto, commit ilegível): o checkout só avança no contrato que já tem, com aviso
  na tela (checkout sem contrato vai ao topo), e o auto-update espera; ele só avança com o topo
  inteiro publicado.
  Evidência em [Atualizar para no binário publicado](#atualizar-para-no-binário-publicado-06102026).
- **App e servidor se entendem por um número de contrato, `CLIENT_API`** (`crates/hangar-api`). O
  servidor responde `GET /api/client-api` → `{"api": N}` (pelo Connect, repassado ao Rust); 404
  (servidor antigo) = 1; 503 `client_api_need_rust_server` (modo `python`) = desconhecido, que nunca
  instala app nem acende "desatualizado". O CI grava `api` em `server-<sistema>.json` e no
  `server-latest.json` (entrada sem o campo ganha o valor do commit dela) e `requires_api` no
  `native-latest.json`; manifesto sem o campo = 1. `scripts/check-client-api.sh` (pre-commit e CI)
  recusa mudança em `crates/hangar-api/src/`, `desktop-native/src/api/` ou literal `"/api/` novo no
  nativo sem subir o número acima do da base; escape `HANGAR_SEM_CLIENT_API: <motivo>` na mensagem.
  O servidor só acrescenta API: remover ou quebrar rota que o app consome exige um ciclo de aviso (a
  rota fica até nenhum app publicado precisar dela). Evidência em
  [Atualização com contrato de API](#atualização-com-contrato-de-api-19102026).
- **O botão de cima do nativo só aparece quando um clique conclui.** App novo com `requires_api` ≤ o
  `api` do servidor no ar: só o app, sem aviso. Exige mais e o servidor publicado para esta máquina
  (`api_publicado` do `GET /api/atualizacao`) atende: servidor primeiro, depois o app. Não atende
  (inclusive branch sem release do servidor): o botão some, sem aviso. Sem servidor local vale o
  `api` do servidor ativo. "Servidor desatualizado" só com `CLIENT_API` do app > `api` do servidor
  ativo, nunca por contagem de commits. Oferta republicada exigindo mais no meio do clique é
  descartada calada e o próximo ciclo refaz o plano.
- **App só instala depois do servidor, e a ordem sobrevive a reabrir o app.** Depois do desfecho
  `Updated`, o app relê `/api/client-api` e só instala se couber; não coube (API desconhecida,
  oferta republicada, instalação manual), o servidor sai como sucesso e o app fica calado para o
  próximo ciclo; o acompanhamento não desiste com o
  motor `rodando` (o teto vale só para leituras seguidas sem resposta) e para em 30 min. Antes do
  `POST /iniciar`, quando há app a instalar depois, grava `update-intent.json` em `appearance::dir()`
  (trava de arquivo contra duas instâncias); a rodada é o desfecho com `estado.ts` posterior à
  intenção, nunca o pid (no Windows o lançador do venv muda o pid). Intenção de outro servidor, com o
  app já igual ou mais novo, ou com desfecho de falha é apagada.
- **A tela pronta vem da chave das fontes, não do commit.** O CI publica `frontend-dist-<chave>.tar.gz`
  e `.sha256` na release `dist-commits` (as 50 mais novas; chave com um dos dois faltando é reenviada);
  a chave é o sha256 do `git rev-parse` de `frontend`, `packages`, `messages`, `project.inlang`,
  `package.json` e `package-lock.json` da raiz, a mesma conta no CI e no motor. Atrás do topo, na main
  e com essas árvores limpas, o motor baixa a da chave e confere o sha256; 404 compila aqui calado,
  outra falha compila aqui e grava `atualizacao.tela_da_chave` no diário. Tela e binário do Rust baixam
  em paralelo, cada um com o seu erro; o binário só é gravado com a tela no lugar (tela que levanta
  erro volta o checkout, e binário novo sob o Python velho recusaria o protocolo). A conferência do Chromium roda no máximo uma vez por dia
  (marca `chromium-ok` com menos de 24 h e sem "falhou" pula).
- **Build do CI: testes ao lado do build, e o push novo cancela o run substituído.** No `server.yml`
  os testes de cada sistema são um job próprio, em paralelo ao build de release, e o `publish` daquele
  sistema só sobe com os dois artefatos dele (`server-<sistema>` e `testes-ok-<sistema>`). Na main e no
  canal, o push novo cancela os jobs de teste e de build do run anterior; o `publish` (Server e Native)
  tem grupo próprio sem cancelamento e nunca é cortado no meio do upload. Cache só a main grava, e quem
  grava apaga as cópias antigas do mesmo prefixo. O LTO completo do `dist` fica. Evidência em
  [CI paralelo e cancelamento](#ci-testes-em-paralelo-e-cancelamento-do-run-substituído-19102026).
- **O app nativo segue o canal do servidor desta máquina.** `pre_voo.alvo` fora da main → release
  `native-<branch>`, que o `native.yml` publica a cada push no canal `beta` (mesma limpeza de nome no
  workflow e no `update.rs`); a `native-latest` continua só da main. Branch sem release, ou sem o
  build da plataforma, mantém o app e avisa na página Sobre, nunca cai na main calada. Trocar de
  canal aceita versão de contagem menor porque o CI embute a branch (`HANGAR_NATIVE_CHANNEL`); build
  local não tem canal e só troca por versão mais nova. Os `install-native.*` seguem na `native-latest`.
- **O botão Atualizar NÃO roda o instalador.** Sozinho ele faz dist do CI, `uv sync`, `npm ci` por
  hash do lock, restart e prova de vida por **pid** (HTTP o processo velho também responde).
  Wrapper/tarefa/statusline só chegam por passo em `docs/atualizacoes/` — o pre-commit e o CI
  recusam commit em `install.*`/`scripts/`/`hooks/` sem passo (`HANGAR_SEM_PASSO=1` é o escape).
  Falha do instalador vai pra tela pela marca `##HANGAR-FALHA##`, nunca pela cauda.
  O `install-native` roda na etapa `nativo`, como o `install-chromium`: script estreito, só com a
  marca, na main e com a API que cabe; falha é aviso. Roda depois da prova de vida, inclusive na
  atualização automática e na topologia manual; não desfaz a atualização do servidor.
- **No Windows, `npm ci` na raiz só com o front parado**, e pasta que o atualizador cria dentro de
  `frontend/` entra no `.gitignore` — sobra não ignorada vira "mudança local" e desliga o dist do CI.
- **Passo com comando diferente por sistema usa `comando_posix` e `comando_windows`.** `comando`
  continua sendo o fallback comum; qualquer variante que executa algo exige `prova`.
- **Passos seguidos com o mesmo comando rodam o comando uma vez; a prova de cada um continua
  conferida**, e a que faltar faz o comando rodar para aquele passo antes de ele falhar. Só o vizinho
  imediato: comando diferente no meio faz o repetido rodar de novo. Em
  02/10/2026 a DELPHI-02 rodou o `install.ps1 -Update` quatro vezes seguidas (~20 s cada) para
  quatro passos da mesma atualização.
- **Versão é `VERSION` + número de commits** (`0.1.0.2533`): major.minor.patch à mão no
  arquivo da raiz, build calculado — nunca tag de release nem commit do CI.
- **Reiniciar o backend**: sem `--reload`; mate `-9` o pid da porta e suba destacado. No Linux é
  `systemctl --user restart`.
- **Reinício pedido pelo app tem estado próprio antes do lançamento.** Se o lançador morrer antes
  do motor escrever, o pid da trava permite publicar a falha para quem pediu; a saída do lançamento
  fica em `logs/privado/atualizacao-launch.log`. HTTP sozinho não confirma reinício: o pid do serviço
  também precisa mudar quando puder ser lido. A tentativa usa `request_id` no POST e no estado:
  o lançador do venv no Windows pode ter pid diferente do motor. Cliente antigo conserva a comparação
  por pid; cliente novo só usa pid quando o servidor não informou o ID. Resposta sem `ok: true` e
  pid válido não inicia a espera.
- **Criar sessão embrulha o tmux em escopo transiente do systemd, sob sonda** — um gerenciador que
  recusa escopo transiente derrubava toda criação de sessão.
- **Atualização cujo motor morreu no próprio reinício é concluída pelo backend novo.** Sem escopo
  transiente, o motor nasce no cgroup do backend e o `systemctl restart` o mata antes de gravar
  "pronto". Nas etapas `reiniciar` e `nativo`, backend com outro pid e disco no `commit_para` =
  sucesso com aviso de que o app nativo não foi atualizado; fora disso, falha.
- **`scripts/verificar-local` é exigido ao abrir PR e ao subir para a `main` ou para branch com PR
  aberto; branch sem PR sobe livre.** O hook PreToolUse do `gh pr create` (`.claude/settings.json`)
  confere o PR inteiro, desde o ponto em que a ponta saiu da base; a trava `pre-push` confere cada
  push, e consulta o PR com prazo de 10 s: sem `gh` ou sem resposta, segue com aviso. PR aberto pelo
  site fica com o CI. O padrão, que as travas exigem, é o rápido: só o ligado aos arquivos mudados
  (`cargo test` do `hangar-server`, ou do workspace quando muda crate de base; pytest dos testes
  mudados e dos que citam o módulo mudado, a suíte inteira quando muda dependência, conftest ou
  helper; `vitest related` e typecheck só com `.ts`/`.svelte`/mensagens no diff, tudo quando muda
  dependência ou configuração; o nativo inteiro, num target próprio da árvore fixa, porque o custo é
  compilar e o target global da máquina fazia o passo esperar a trava dos builds de quem programa);
  Windows só com `.ps1`, `procinfo.py` ou `atomico.py` no diff, e a VM nunca passa de 10 min
  (`HANGAR_VERIFICAR_WINDOWS_MAX`). O passo completo gravado cobre o rápido da mesma área; o rápido
  só vale para base igual ou mais velha que a pedida. Medido no PR da voz (`639c486a79`): 454 s
  (rust 453 s, sendo 297 s de compilação; nativo 356 s, ~150 s esperando a trava; front 246 s),
  contra ~33 min do Linux completo.
  `--completo` roda o que o CI rodaria para o que o commit muda, num clone fixo por máquina (`~/.cache/hangar-verificacao/arvore`), em fila
  única (`flock`) e prioridade baixa, com HOME vazio e o `omp` fixo do CI; Windows por SSH na VM
  de `git config hangar.verificarWindows`, com mutex. O registro é pelo tree sha; a trava só o lê.
  Escape nos dois: `HANGAR_SEM_VERIFICACAO=1`. Na VM roda só o que o CI roda no Windows (Rust e os 6
  `test_runtime_*`), nunca o pytest inteiro. O passo `mobile` só entra quando muda `mobile/`: o CI
  não testa o app, e mudança no core ou nas mensagens já passa pelo `front` (à mão:
  `--tudo --passos mobile`). Evidência em
  [Verificação local antes do push](#verificação-local-antes-do-push-06102026).
- **No Linux a verificação prepara uma vez e roda os passos em duas faixas (cargo e o resto); o
  registro é por passo.** Faixa que abre com a memória disponível abaixo de
  `HANGAR_VERIFICAR_MEM_GB` (padrão 6) espera a anterior terminar, com os mesmos passos. Quando o
  core muda, o rápido do front roda os testes dos arquivos do front que importam um nome que o diff
  do core mudou, direto ou por quem o usa dentro do core (`scripts/core-front-tests.mjs`); o do
  backend escolhe os testes por quem importa o módulo. O preparo é o único que compila e instala, e o pytest roda com `pytest-xdist`, um arquivo
  por processo (`git config hangar.verificarProcessos`, padrão 4; `1` = em fila). Um passo que passou
  fica gravado para a árvore, e a rodada seguinte só repete o que falta. O Node é o do `.node-version`
  da árvore verificada. Teste do backend não pode deixar timer, thread que age, `sys.modules`,
  módulo recarregado ou tratador de sinal alterado para o arquivo seguinte: com a suíte repartida, quem paga é o vizinho. Evidência em
  [Verificação local em paralelo](#verificação-local-em-paralelo-09102026).
- **Build e teste locais: target comum, binário fixado na worktree, mensagens com carimbo.** Toda
  worktree compila no target comum da máquina pelo `scripts/cargo-pinned`, que pega a trava
  `/tmp/hangar-build.lock` e copia os executáveis para o `crates/target` do checkout na mesma trava:
  pytest e `CP_RUST_SERVER_BIN` rodam essa cópia, nunca o executável do target comum. A troca em
  `~/.hangar/bin` usa o release do target aquecido; o perfil dev não é mais rápido na mudança
  incremental. O `i18n:compile` de front, core e app passa por `scripts/i18n-compile.mjs` e pula
  quando entrada e saída batem com o carimbo da última compilação. Evidência em
  [Build e teste locais rápidos](#build-e-teste-locais-rápidos-23102026).
- **Build Rust: testes de integração do `hangar-server` num executável só, e o CI publica com LTO
  fat só na `main`.** Teste novo entra como módulo de `crates/hangar-server/tests/it/`; teste que
  mexe em estado do processo inteiro (PATH, HOME, TZ, logger global, gancho de pânico) vai para um
  `[[test]]` à parte no `Cargo.toml`. Quem reexecuta o próprio executável monta o nome com
  `exact_name(module_path!(), …)` e confere `assert_ran_one`; filho de `fork` sem `exec` chama
  `close_inherited_fds`; porta que deve recusar vem de `refused_address`; porta que deve ter fechado
  passa por `assert_closed`; lease ou porta reaberta logo depois de solta, por `lease_when_free` ou
  espera com prazo; executável de teste, por `write_executable`. A depuração reduzida vem do perfil
  `dev` do `crates/Cargo.toml`, nunca de variável, e o `hangar-server` pede as mesmas features que o
  resto do workspace: `-p` e `--workspace` têm de dar o mesmo hash. O binário publicado pela `main`
  sai do perfil `dist` (`--profile dist`, em `target/dist/`); a branch, do `release`. O cache tem o
  perfil na chave e só guarda dependências de terceiros (`scripts/podar-target --cache`). Evidência em
  [Build Rust e CI menores](#build-rust-e-ci-menores-09102026).
- **CI do Rust: testar em toda branch, compilar release e publicar só na `main` e no canal `beta`.**
  Nas outras branches o `server.yml` roda só os testes e o `native.yml` só o `check` (build e test
  em debug, Linux); push novo cancela a conferência do anterior. `workflow_dispatch` numa branch
  compila sem publicar. O `publish` do nativo espera o `check`. Trocar o canal é trocar `beta` nos
  dois arquivos. Evidência em
  [CI do Rust: publicar só na main e no canal](#ci-do-rust-publicar-só-na-main-e-no-canal-05102026).
- **O bloco do MCP `hangar` no `config.toml` do Codex é reconhecido pela TABELA, não só pelos
  marcadores.** O app desktop reescreve o arquivo sem comentários; quem só procura `# >>> hangar`
  anexa de novo, e o TOML com chave duplicada derruba o ChatGPT e o Codex juntos.

### Pacote por versão

Máquina migrada roda de um pacote; as regras do motor `git` acima valem só para o modo
desenvolvedor e para a máquina ainda não migrada. Evidência em
[Pacote por versão em vez de `git pull`](#pacote-por-versão-em-vez-de-git-pull-24102026).

- **Uma versão é uma pasta que nunca muda depois de extraída.** `~/.hangar/versions/<versao>/`
  com `manifest.json` (`version`, `commit`, `channel`, `protocol`, `api`, `os`, `dist_key`, sha256
  por arquivo); `~/.hangar/current` aponta para a versão em uso. O bytecode vai para
  `~/.hangar/cache/pycache/`, e o pacote leva os `.pyc` em `unchecked-hash`: um `__pycache__` dentro
  da versão faz a reextração recusar a pasta (`package_existing_differs`).
- **Dados do usuário moram em `~/.hangar/config/`** (`.env`, `peers.json`, `projects.json`,
  `launchers/`). Quem lê procura lá primeiro e só então em `backend/` (máquina não migrada);
  quem grava grava no caminho que leu. Instaladores e `services-setup.sh` seguem a mesma regra.
- **Serviço e processo longo rodam do caminho real da versão; o que é gravado fora do Hangar
  aponta para o `current`.** Unit e tarefa chamam o lançador estável
  (`~/.hangar/bin/hangar-launcher launch backend`), que lê o `current` uma vez; o `rust_bins` e todo
  `sys.executable` gravado usam o caminho real. Hooks, atalhos, `plugin-dir`, skills e `⟦HANGAR⟧`
  gravam `~/.hangar/current` sem resolver o link, e toleram a pasta ausente por um instante.
- **Script do Hangar roda o Python pelo atalho `hangar-python`**: o embutido do pacote, senão o venv
  do backend, senão o `python3` da máquina. O pacote não leva Node nem pip; Node é dependência da
  máquina, garantida por passo de máquina.
- **O pacote só sai de commit com o `ci.yml` (job `backend`) e os testes do `server.yml` daquele
  sistema verdes.** O `package.yml` publica em `package-latest` (main) ou `package-<branch>`, com o
  índice `package-<canal>.json` por último; sistema não pronto fica com a entrada anterior.
  Dependência sem wheel vira wheel no CI, a partir do sdist conferido por hash.
- **Extração conferida.** O extrator (`crates/hangar-package`) extrai em `versions/<versao>.tmp/`,
  confere o sha256 de cada arquivo e só então renomeia para o nome final, com novas tentativas
  enquanto o sistema diz "em uso"; pasta incompleta nunca tem o nome de versão. Manifesto acima de
  4 MiB é recusado. Pasta existente que não confere é refeita, menos a do `current`.
- **Na máquina migrada, atualizar é do Rust (`crates/hangar-server/src/update/`).** Índice →
  download com sha → extração → tela web da `dist_key` (falha é aviso) → só os passos
  `tipo: maquina` → executor destacado → troca do `current` → reinício → prova de vida → poda.
  Falha antes da troca não muda nada; depois dela, o `current` volta e o servidor reinicia. A prova
  é o `/__hangar_server/health` com o `protocol` e o `commit` do manifesto novo e pid diferente:
  o Python sozinho na porta reprova. Ficam a versão em uso e a anterior; a poda pula pasta com
  processo vivo e apaga por `rename` para `versions/.trash-*`. A verificação automática roda de hora
  em hora, sem `git`.
- **Passo de máquina tem de funcionar com a versão anterior**: a volta não o desfaz. Script,
  skill, hook e atalho chegam com o pacote e deixam de ser passo.
- **Modo desenvolvedor: o `current` aponta para um checkout sem `manifest.json`.** Atualizar é
  `git pull --ff-only` na branch dele e reiniciar, sem recompilar; árvore editada, sem upstream ou à
  frente recusa com o motivo. O laço automático do Python segue no motor `git` ali, e o executor do
  Rust recusa rodar junto (`OtherEngine`). Depois da migração, `hangar-server dev-mode on <checkout>`
  liga e `off` volta para a versão instalada mais nova deste sistema (sem nenhuma, recusa), com a troca,
  o reinício e a prova do atualizador; servidor que não sobe faz o `current` voltar. O checkout não
  tem `bin/`: o Rust vem do `~/.hangar/bin`, atualizado pelo `app.rust_release` do venv do checkout
  antes da troca no `on` e depois do `git pull` de cada atualização; falha é aviso e não para nada.
- **A migração (`hangar-server migrate-package`) roda uma vez, fora do serviço, e se desfaz.**
  Checkout editado, com arquivo novo, fora da `main` e da `beta`, sem upstream, à frente ou servido
  pelo `vite` (`SERVE=preview`) vira modo desenvolvedor; o resto vira pacote do canal da branch
  (`beta` → `package-beta`, `main` → `package-latest`). Ela para o backend antes de mover os
  dados, registra cada efeito (`~/.hangar/migration.partial.json`, depois `migration.json`), desfaz
  tudo se falhar antes da prova e recusa rodar dentro do cgroup ou do Job do backend. `--undo`
  reaponta tudo para o checkout, que nunca é apagado.
- **Instalação nova no Linux é pelo pacote; `--dev` é pelo git.** O `bootstrap.sh` baixa o
  `hangar-server` da `server-<canal>` (sha256 do `server-latest.json`), roda `hangar-server
  install-package --channel <canal>` e depois o `install.sh` do `current`. O `install-package` reusa o
  download da migração, copia o `bin/hangar-server` do pacote para o lançador, registra todo passo
  como aplicado e grava a marca `migration.json` (pacote, sem checkout) antes do `current`; recusa
  (4) com `current`, migração, checkout anotado ou unit já no lugar, e sai 3 sem pacote para o
  sistema, quando o bootstrap segue pelo git. O `install.sh` com `manifest.json` e sem `.git` pula
  git, `uv`, venv, build da tela e binários Rust, grava o `.env` em `~/.hangar/config` e põe um
  `python3` que é o `hangar-python` à frente do PATH dos sub-scripts. O `--dev` grava
  `~/.hangar/dev-requested`, e a migração lê isso como modo desenvolvedor.
- **A tela web é opcional, ligada por padrão, e servida pelo Rust** de `~/.hangar/ui/<dist_key>/`
  (checkout: `frontend/dist`). `GET`/`PUT /api/web-ui` (só o dono) grava `~/.hangar/web-ui.json`;
  desligada, a raiz responde 404 `web_ui_disabled` e o atualizador não baixa a tela. O interruptor
  fica no nativo e no Expo: desligar pelo PWA desligaria o próprio PWA.

## Pacote por versão em vez de `git pull` (24/10/2026)

Motivo: em 24/10 a DELPHI-02 e o PC do Rafael pararam com versão misturada, código Python de um
commit com binário Rust de outro. Com o pacote, código, Python e binários saem juntos do mesmo commit
testado, e a troca é um apontador. Decisões do Jefferson no mesmo dia: Python embutido; Linux e
Windows juntos, macOS depois; tela web opcional; quem desenvolve continua editando (modo
desenvolvedor); atualizador no Rust; versão `VERSION` + número de commits. Plano:
`docs/superpowers/plans/2026-10-24-pacote-por-versao.md`.

Medido nas provas de base (Task 1, Linux e DELPHI-02):

- **Python embutido.** `python-build-standalone` `20261021`, CPython 3.14.9: no Linux exige no
  máximo `GLIBC_2.17`, dentro do alvo 2.28 (`manylinux_2_28`, o mesmo do `hangar-server`). Das 55
  dependências do `backend/uv.lock`, só `http-ece==1.2.1` não tem wheel; é Python puro, e o CI gera o
  `py3-none-any` uma vez. `import app.main, app.api` sobe no Python embutido com `HOME` vazio.
- **Pasta por link.** No Windows um processo vivo carrega pela junction o código da versão nova; no
  Linux o prefixo do Python é resolvido, mas `PYTHONPATH` e filhos lançados por `sys.executable`
  seguem o link. Por isso serviço, lançador e todo `sys.executable` gravado usam o caminho real.
- **`resources/` vai na raiz do pacote**: `transcribe.py` o lê na importação.
- **Node fica na máquina** (statusline do Claude, `hangar-preview`, `folha`); a aceitação "sem `git`,
  `uv`, `npm` ou `winget`" não inclui o Node. Máquinas em `SERVE=preview` (esta e a VPS) viram modo
  desenvolvedor, sem mudar endereço nem o que o navegador guardou; a VPS segue no `deploy.sh`.

Medido ao montar o pacote (Task 6):

- **Tamanho do pacote de Linux: 123 MB** com o Python `install_only_stripped`, contra 216 MB com
  símbolos. O de Windows também monta no Linux (`uv pip install --python-platform`).
- **Teto do manifesto (4 MiB).** Só a `terminfo` do standalone eram 2871 arquivos e levavam o
  manifesto acima do teto; `include/` e `share/` do Python saem do pacote, porque o backend não os usa.
- **pip fora.** O pacote não instala nada em tempo de execução; o pip do standalone é removido.
- **`.pyc` em `unchecked-hash`.** Rodando o pacote, o Python gravou `__pycache__` dentro de
  `versions/<v>/`, e a reextração caiu em `package_existing_differs`. O lançador põe
  `PYTHONPYCACHEPREFIX` fora da versão, mas hooks (`preview_hook.py` importa `app.atomico`) rodam o
  Python do pacote sem ele; com os `.pyc` prontos e listados no manifesto, o Python só os lê.
- O extrator aceita o pacote montado, e `import app.main, app.api, http_ece` sobe com `HOME` vazio.

Atualizador (Task 7): `cargo test -p hangar-server update` com 61 testes da lib, 1 e 9 de integração;
tela web, 11. No Windows o comportamento foi só raciocinado.

**Medição da Task 11 pendente.** Faltam as provas reais: atualizar pelo app esta máquina (modo
desenvolvedor) e uma máquina Linux em pacote; migrar e atualizar a DELPHI-02 em pacote, com falha
forçada no reinício (volta inteira, Rust de pé, sessões com e sem terminal reportando); máquina sem
tela web atualizando sem baixar o `dist`, com Expo e nativo funcionando. Também falta o Step 3 da
Task 6 (`gh workflow run package.yml` publicar os dois sistemas e o pacote de Linux subir num
contêiner sem Python, uv e Node). O passo `docs/atualizacoes/2026-10-24-pacote-por-versao.md` só
entra na `main` depois dessa prova: um passo na `main` roda em toda máquina no próximo Atualizar.

## App nativo na atualização do servidor (21/10/2026)

Atualizar várias máquinas precisa trocar também o arquivo do app nativo em cada uma: o botão do
app aberto só alcançava a própria máquina. O motor agora chama `install-native.sh` ou
`install-native.ps1`, sem o instalador completo, depois da prova de vida do servidor e antes de
concluir a atualização. A marca `~/.hangar/native/release.json` sem `sem_build` evita instalar app
em quem não o usa; só a main entra porque esses scripts leem `native-latest`.

Os scripts não conferem `requires_api`. Por isso o motor lê o manifesto e consulta, com a
credencial do dono, `/api/client-api` no loopback da porta atualizada: API desconhecida ou menor
que a exigida apenas registra o motivo no diário e conserva o app. Manifesto sem `requires_api`
e servidor antigo com 404 valem API 1, pelo contrato existente. Falha do script, inclusive prazo
de 900 s excedido, entra em `estado.avisos` e conserva `ok=True`: o servidor já está atualizado,
e voltar seu código por causa do app quebraria a ordem servidor → app.

Limite desta guarda: o script baixa o manifesto novamente. Se `native-latest` for republicada
entre a conferência do motor e essa leitura, a oferta instalada pode exigir outra API. Fechar
essa janela exige que o script use a mesma oferta conferida; os scripts não mudam nesta Task.

O app aberto continua com a imagem anterior até ser reaberto. Se o motor morrer nas etapas
`reiniciar` ou `nativo`, o backend novo mantém a prova de pid diferente e `commit_para`, conclui o
servidor como atualizado e acrescenta o aviso do app não confirmado. Não tenta instalar de novo
dentro da leitura de estado. A conferência real na DELPHI-02 não pôde ser feita nesta execução:
`ssh delphi-02` respondeu `No route to host`; nem a presença do checkout foi confirmada.

## Restarting the backend.

No `--reload` (it holds SSE + watchfiles). `pkill -f app.main` can match your
  own shell; SIGTERM can hang on an open SSE connection. Kill `-9` the pid bound to the port and relaunch
  detached (`setsid`).

## Instalação seletiva dos workspaces

Registro trazido do `CLAUDE.md` em 11/09/2026. O comando web, no CI e nos instaladores, é
`npm ci --workspace=@hangar/core --workspace=frontend`. O EAS Build detecta o monorepo pelos
`workspaces` da raiz; retirar `mobile` fazia o envio conter só o app, deixando
`@hangar/core` (`file:../packages/core`) como link quebrado. Reproduzido com
`git archive HEAD mobile` e `npm install` em pasta isolada: a instalação saía 0, mas o bundle
falhava com `Cannot find module '@hangar/core'`.

Na medição registrada, a instalação seletiva incluiu 170 pacotes, contra 167 quando o app
estava fora dos workspaces. O lock da raiz passou de 7617 para 14348 linhas porque descreve
todos os workspaces; isso não significa instalar React Native no caminho web. Para desenvolver
o app, `npm install` dentro de `mobile/` usa seu lock próprio. O `metro.config.js` declara
`watchFolders`/`nodeModulesPaths`, e o `react-dom` dos testes acompanha a versão de `react`.

## Quem serve a interface é o BACKEND, e o `frontend/dist` chega pronto do CI.

Duas mudanças de
  25-29/08/2026 que andam juntas, e as duas têm o mesmo antônimo: uma máquina de quem usa não
  compila nem serve nada além do necessário.
  - O backend monta o `frontend/dist` na raiz (`api.py`, `_UIStatic`), então o `vite preview` num
    serviço à parte era um SEGUNDO servidor para o mesmo arquivo. Instalação nova não registra mais
    esse serviço — no Linux o `services-setup.sh` já decidia assim; o `install.ps1` passou a seguir.
    Quem **já** tem o serviço fica com ele: trocar a porta muda a ORIGEM, e origem nova é
    `localStorage` vazio (`cp_servers` com os tokens, tema, layout do canvas). Ninguém perde
    configuração por causa de um `git pull`. O que decide é a existência da unit/tarefa, nunca uma
    pergunta nova.
  - `CP_FRONT_PORT` deixou de ter `5173` cravado como default (`config.porta_do_front`): vazio = a
    porta do próprio backend. Com o serviço do front fora, o QR e o painel de alcance apontavam para
    uma porta onde ninguém escuta — foi o `Rede local … não respondeu` do painel. Quem mantém o
    preview tem o `5173` **gravado** pelos instaladores, e é isso que preserva a origem dele.
    O firewall também segue essa decisão: a 5173 só é liberada quando há serviço de front.
  - O `ci.yml` publica `frontend-dist.tar.gz` + `frontend-dist.sha` na release fixa `dist-latest` a
    cada push na main, e os instaladores baixam de lá **só** quando o `.sha` bate com o `HEAD` e o
    `frontend/` não está editado. Não bateu (CI ainda compilando), sem rede, ou tar quebrado → build
    local, como sempre foi. `tar.gz` e não zip porque o Windows 10+ traz `tar.exe` — um comando só
    nos dois instaladores. O `npm ci` **continua** para quem mantém o preview, que precisa do
    `node_modules`.
  - **Os dois gates dos instaladores perguntam pelo `packages/` junto do `frontend/`, e essa
    palavra é a única coisa que os separa de servir tela velha** (09/09/2026, quando os 46
    arquivos saíram de `frontend/src/lib` para `packages/core`). A tela passou a ser buildada a
    partir de DUAS árvores, e os dois gates ainda perguntavam por uma:
    - *Precisa rebuildar?* (`install.sh`, por mtime) — `find frontend/src …` não olhava o core, e
      um `git pull` que mexesse só em `api.ts`/`format.ts` respondia "já buildado e atualizado",
      servindo o dist velho, calado. Reproduzido em sandbox: com o core alterado, a lista antiga
      diz "já buildado" e a nova diz "precisa rebuildar". Hoje o `find` leva `packages/core/src`.
    - *A pessoa está editando o front?* (`install.sh` e `install.ps1`, por `git status
      --porcelain`) — cego para o core, ele baixava o dist do CI **por cima** de uma edição local
      em `packages/` e apagava da tela o que ela estava escrevendo. Efeito oposto ao de cima, mesma
      causa. Hoje o pathspec é `-- frontend packages`; em repositório descartável, com só o core
      editado, `-- frontend` devolve vazio e `-- frontend packages` devolve a linha do arquivo.
      No `install.ps1` esse mesmo valor (`$sujo`) alimenta os dois gates, porque a `$marca` de
      rebuild é `commit HEAD` + `$sujo`.
    Duas coisas medidas que o conserto NÃO precisou tratar: `find` com um dos caminhos ausente
    (checkout pré-migração) erra em stderr só naquele argumento e segue avaliando os outros, então
    não vira falso "não precisa"; e `packages/core/src/paraglide/` é gerado e gitignored, e
    `git status --porcelain` sem `--ignored` não o lista — ele não dispara rebuild à toa.
    A regra que sobrevive à próxima mudança de layout: **gate que pergunta "o front mudou?" tem
    que conhecer todas as árvores de onde o front é compilado.**

## Session creation's systemd-scope probe.

Creating a session wraps `tmux` in
  `systemd-run --user --scope` so the tmux server doesn't inherit the backend's cgroup, but the wrap
  is now gated on a probe: a systemd user manager that refuses transient scopes was making **every**
  session creation fail (app and terminal both). Failing the probe, sessions are created without the
  scope and the backend logs a warning (commit `23da052`).

## Atualizar pelo app

(`app/atualizar.py` + `app/atualizacoes.py` + `docs/atualizacoes/`): o
  botão faz tudo sozinho — decisão do usuário em 25/08/2026 —, `reset --hard` e reinstalador
  incluídos, porque quem usa não administra nada e não deve precisar saber que passos existem.
  Quatro coisas que o desenho decide de propósito:
  - **Roda destacado do backend** (`setsid` / `DETACHED_PROCESS`), e o progresso mora em
    `<config>/.hangar-update/estado.json`. A atualização reinicia o backend: dentro do processo ela
    se mataria no meio, e a máquina ficaria com código novo no disco e processo velho no ar — o
    estado que `install.ps1:1242` já registra como o pior. O arquivo é também o que deixa a tela
    dizer "atualizando…" enquanto o servidor volta, em vez de "desconectado".
  - **Automático não é irreversível.** `resguardar()` roda antes de qualquer coisa destrutiva: o
    que estava no disco vai pra `resgate/<data-hora>` + stash, e a função **confere a ref** antes
    de devolver. Falhou o resgate, a atualização para com o disco intacto. O único `reset --hard`
    que não passa por ali é o rollback, cujo alvo é um commit da própria máquina de minutos antes.
  - **O registro é do que JÁ RODOU aqui** (`aplicados.json`), não do intervalo de commits. O
    intervalo fura em instalação nova e em quem reclonou ou resetou. Instalação do zero marca tudo
    como aplicado (os dois installers), senão a primeira atualização roda a história inteira.
  - **Passo só entra no registro depois da PROVA passar** — comando com exit 0 e efeito ausente é
    a falha que o campo `prova` existe pra pegar. Passo novo: um arquivo em `docs/atualizacoes/`
    (formato no README de lá), no mesmo commit que o exige. Os não destrutivos também rodam na
    **subida do backend** (`main.py`), pelo motivo do `migracao_sidecars`: atualizar aqui é
    `git pull` + reiniciar, e ninguém garante que o botão foi usado.
  - **Quem reinicia o serviço é diferente em cada sistema, e no Windows já é o installer.** No
    Linux é `systemctl --user restart`; no Windows o `install.ps1 -Update` — chamado na etapa
    anterior — já derruba a instância velha (`Restart-HangarTask`) e chama `Start-ScheduledTask`, e esse
    bloco NÃO é pulado no modo `-Update` (o que ele pula é firewall/Tailscale e o hook). Há ainda
    a tarefa `hangar-vigia`, que confirma falha HTTP e recupera a instância identificada. Por isso
    `_reiniciar` não faz nada no ramo Windows: marcar "falta reiniciar" ali fazia a tela pedir um
    passo que já tinha sido dado. Medido em 25/08/2026 naquela máquina: três tarefas
    (`hangar-backend`, `hangar-frontend`, `hangar-vigia`), backend como cadeia de três processos,
    e todas em `Ready` mesmo com o servidor vivo — o `.vbs` não esperava. Esse lançador foi
    substituído pelo acompanhamento descrito abaixo.
  - **O installer matava a própria atualização, e a proteção é por COMANDO, não por linhagem.** O
    `Pare-Servico` derruba a "instância anterior" casando o caminho do checkout mais `uv|python`, e
    o motor roda como `<repo>\backend\.venv\Scripts\python.exe -m app.atualizar` — casa nos dois.
    Ou seja, o instalador chamado PELA atualização matava quem o invocou, no meio dela (medido
    25/08/2026: lock e processo morreram no minuto do "instância anterior derrubada"). A proteção
    por linhagem já existia e não bastou; hoje há exclusão explícita de quem tem `app.atualizar` na
    linha de comando. No Linux quem cobre isso é o escopo transiente do systemd, que lá não existe.
    **E a linhagem tinha o furo oposto (09/09/2026):** como a cadeia do app é `backend
    (python -m app.main) → app.atualizar → powershell install.ps1 -Update`, o backend VELHO era
    ancestral do instalador e, protegido por isso, ficava de fora dos alvos — `porta 8765 continua
    ocupada (pid N) apos parar hangar-backend` em toda atualização pelo app, com código novo no
    disco e servidor velho no ar. Hoje a subida da linhagem PARA no motor (cmdline casando
    `app.atualizar`): ele entra na linhagem, os pais dele não.

## O botão Atualizar não roda o instalador; o instalador só roda por passo declarado

(`atualizar._preparar`, `scripts/check-passo-de-atualizacao.sh`, job `passos` do CI, 14/09/2026.)
O motor chamava `install.ps1 -Update` / `install.sh --update` em toda atualização — 8 etapas, 24
ramos `if ($Update)` só no `.ps1`, cada um uma chance de falhar sem ninguém ter pedido nada dele.
Foi assim que usuários Windows receberam "NAO terminou: tarefas agendadas, backend no ar" num
`git pull` que não mudava tarefa nenhuma: o commit `41c1a0ff` ("Windows runtime remains
unverified") pôs `throw` de RunLevel/RestartCount no passo 7/8 que estouravam numa tarefa
reaproveitada com a recuperação pausada pelo passo 0 — ANTES do `Restart-HangarTask`, então o
processo velho seguia na porta com o código novo no disco. E o modal mostrava as 12 últimas linhas
(o portão do fim), com o motivo acima do corte.
  - **O caminho padrão é o que o `git pull` não traz**: dist do CI, `uv sync` (sempre — idempotente
    e rápido com o lock igual; comparar `de..para` seria o intervalo de commits, que mente em
    máquina reclonada), `npm ci` só quando o hash do `package-lock.json` gravado no sidecar mudou
    e já há `node_modules`, restart, prova de vida.
  - **Wrapper, tarefa, statusline, hangar-send só chegam por passo em `docs/atualizacoes/`**, com
    o comando cirúrgico da área (tabela no README de lá). O pre-commit e o CI recusam commit que
    toque `install.*`, `scripts/` ou `hooks/` sem passo novo; `HANGAR_SEM_PASSO=1` é o escape.
  - **O `-Update` continua existindo e tendo que funcionar**: é o hook post-merge de quem atualiza
    por `git pull` na mão, e o que um passo chama quando não há script por área (Windows).
  - **Prova de vida por pid**, não só HTTP: o processo velho responde `< 500` igual. No systemd é
    o `MainPID` da unit; no Windows é `psutil.net_connections` na porta. Pid igual = rollback.
  - **O dist anterior fica em `frontend/.dist-velho` até o fim** e volta no rollback junto com o
    código — antes o rollback deixava o backend velho servindo a tela nova.
  - **No Windows quem reinicia é `Restart-HangarTasks` do `windows-tasks.ps1`** (mesmo mutex
    `Local\HangarInstall` do instalador e da vigia), chamado pelo motor. O `.ps1` é lido do disco
    depois do pull, então o conserto chega pela própria atualização quebrada.
  - **Falha do instalador chega à tela pela marca `##HANGAR-FALHA##`** (irmã da `##HANGAR-AVISO##`),
    impressa por `Falha`/`Pare` no `.ps1` e por `fail` no `.sh`; a cauda de 12 linhas é só o
    fallback sem marca.

## No Windows o `npm ci` do botão derruba o front antes, e as sobras do dist são ignoradas pelo git

(`atualizar._stop_windows_front`, `Stop-HangarFrontend` do `windows-tasks.ps1`,
`frontend/.gitignore`, 01/10/2026.) Três defeitos encadeados deixaram uma máquina Windows sem
conseguir atualizar, nem pelo botão nem pelo instalador:
  - **O `_preparar` rodava `npm ci` com a tarefa `hangar-frontend` de pé.** O `vite preview` dela
    mapeia `node_modules\@rolldown\binding-win32-x64-msvc\rolldown-binding.win32-x64-msvc.node`, e
    o `npm ci` morria em `EPERM ... unlink` (errno -4048) depois de apagar o resto — `node_modules`
    sem `.bin`, front vivo só da imagem em memória. Como a marca `package-lock.sha` só é gravada no
    sucesso, toda tentativa repetia o `npm ci` e a mesma falha (12:25, 12:59 e 13:28). O
    `install.ps1` já derrubava o front antes do `npm ci` desde 08/08/2026; o caminho do botão,
    criado depois, não. Hoje o motor para os processos da tarefa antes, o `_reiniciar` a sobe, e
    na falha do `npm ci` a tarefa é iniciada de volta.
  - **`frontend/.dist-velho` não era ignorada pelo git.** Ela só sai no sucesso; a falha acima a
    deixou no disco, e `git status --porcelain -- frontend packages` passou a devolver
    `?? frontend/.dist-velho/`. Os dois gates leram isso como "frontend editado", descartaram o
    dist do CI e foram compilar local. `.dist-velho/` e `.dist-baixado.*` estão no
    `frontend/.gitignore`.
  - **O lock da raiz só tinha o `@rollup/rollup-linux-x64-gnu`** (gerado no Linux, bug 4828 do
    npm). O `vite build` passa — o Vite usa rolldown —, mas a etapa do service worker
    (`workbox-build`, que usa o rollup) morria em `Cannot find module
    @rollup/rollup-win32-x64-msvc`. A entrada do pacote win32 foi acrescentada à mão no lock.
    Quem regenerar o lock no Linux confere se ela continua lá.
  - **O passo desta mudança para o front, e é `destrutivo: true` só para não rodar na subida.**
    O motor carrega o `atualizar.py` antes do `git pull`, então a atualização que ENTREGA este
    conserto ainda roda o `_preparar` antigo — e como ela também muda o lock, cairia no mesmo
    `EPERM`. Os passos rodam depois do pull e antes do `npm ci`: o comando Windows do passo chama
    `Stop-HangarFrontend`, e o `_reiniciar` sobe a tarefa. Na subida do backend ninguém a subiria
    de volta, por isso o passo espera o botão.

## Passo com comando por sistema (14/09/2026)

Um único `comando` não expressava a própria tabela de `docs/atualizacoes/README.md`: reinstalar
`hangar-send`, skills e o bloco global usa `./scripts/install-hangar-send.sh` no POSIX e
`install.ps1 -Update` no Windows. Rodar o primeiro pelo `cmd.exe` falha antes da prova; rodar o
segundo em Linux nem existe. O frontmatter agora aceita `comando_posix` e `comando_windows`, com
fallback em `comando`. A variante da plataforma é escolhida ao ler o passo, e qualquer uma das
três chaves exige `prova`. O teste lê o mesmo passo simulando as duas plataformas e confirma o
comando efetivo.

## Versão é `VERSION` + número de commits (`0.1.0.2533`)

(`VERSION`, `diag.versao_legivel`, `vite.config.ts`, 14/09/2026.) `pyproject` ficou em 0.1.0 e
`package.json` em 0.0.0 para sempre: número que só sobe à mão apodrece. A primeira versão desta
regra era data + hash (`2026.09.14-360978f7`), e o usuário recusou: "isso não é versão". O
formato pedido é `major.minor.patch.build`: os três primeiros vêm do arquivo `VERSION` na raiz
(mexido à mão quando ele quiser marcar algo) e o `build` é `git rev-list --count` — sobe sozinho
a cada commit na `main`, sem tag e sem commit do CI, e é comparável entre disco, processo e
`origin/main` (o `VERSION` é lido do próprio ref, `git show ref:VERSION`, porque o remoto pode
ter um bump que o checkout ainda não puxou). `GET /api/atualizacao` traz as três em
`versao_legivel` (`repo`, `backend`, `remoto`) e `atras`; a barra do desktop e o rodapé do
celular mostram `v0.1.0.2533` com o tooltip dizendo atualizado / N atrás / falta reiniciar.
Sem git, o campo é `null` — palavra fixa virava `vindisponivel` na tela.

## Instalador com portão de prova por etapa

(`install.sh` / `install.ps1`, 02/09/2026). Duas
  gravidades: **essencial falhou → para na hora** com causa e conserto (`fail` / `Pare`): deps,
  backend (`uv sync`), token (releitura do `.env`), frontend (rc + `dist/index.html` existe).
  **Extra opcional que a pessoa pediu falhou → entra na lista** (`PROBLEMAS` / `$pendencias`) e o
  fim diz "terminou com pendências"/"NAO terminou", nunca "Pronto". Motivo: instalações saíram
  "Pronto" com o `tailscale serve` quebrado (HTTPS não habilitado no tailnet) porque a falha só
  imprimia amarelo no meio da tela — o `serve` agora tem **prova** (relê o `serve status` e exige
  a raiz do `:443` na porta do backend; `Get-Proxy443`, reusada da detecção). Parar no meio por um
  extra trancaria a instalação de quem nem consegue habilitar HTTPS no tailnet, então o extra vai
  pro portão do fim, não pro stop. No `--update`/`-Update` os extras seguem moles
  (`##HANGAR-AVISO##`): falhar ali derrubaria a atualização inteira do app por causa de um extra.
  Cara nova: banner, barra `[###-----] N/8` nos títulos numerados (dentro de `say`/`Titulo`, sem
  mexer nas chamadas), spinner nos comandos longos do sh (`gira` — sem TTY ou `--update`, passa
  direto com saída ao vivo), caixa RESUMO no fim (token só com TTY, mesma regra do passo 3/8).

## A tarefa Windows acompanha o processo até ele terminar

(12/09/2026). O `.vbs` usa `Run(..., 0, True)` e devolve o código do filho com `WScript.Quit`.
Backend e frontend legado usam `MultipleInstances=IgnoreNew`, `RestartCount=3` e intervalo de
um minuto; permanecem no logon interativo, com o nível de permissão escolhido na instalação.
A vigia também aguarda seu PowerShell, impede sobreposição e limita cada execução a dois minutos.

`scripts/windows-tasks.ps1` compartilha a recuperação entre instalador e vigia. A vigia faz
duas tentativas HTTP de três segundos, separadas por dois segundos; aceita respostas abaixo de
500, inclusive 404 sem o dist, e respeita dez minutos desde o início da tarefa/processo.
Não é uma verificação funcional de todas as rotas. WMI indisponível, PID não identificado,
porta de outro processo ou processo que não encerra impedem a criação de outra instância.

`Restart-HangarTask` encerra só os processos do serviço identificados pelo checkout/lançador,
confere nascimento do PID (tolerância inferior a um milissegundo entre WMI e GetProcessTimes),
aguarda a tarefa sair de `Running` e a porta ficar livre, e só então inicia novamente. Não usa
`Stop-ScheduledTask` nem mata toda a árvore: psmux e `app.atualizar` não são alvos.

**Não** pausa o reinício automático (12/09/2026): zerar o `RestartCount` por `Set-ScheduledTask`
gera `<RestartOnFailure>` com `<Interval>` e sem `<Count>`, e o Agendador recusa o XML inteiro —
`HRESULT 0x80041319`, "Um elemento ou atributo necessário está faltando no XML da tarefa.
(43,8):Count:". Como as tarefas nascem com `RestartCount=3`, isso falhava em **todas** as
chamadas: o passo 7/8 morria antes de iniciar a tarefa (pendência "nenhuma tarefa chegou a ser
iniciada", instalador em exit 1) e a vigia nunca recuperava travamento nenhum. Além de
impossível, era desnecessário — o reinício por falha só dispara um minuto depois da morte do
processo, e aí a nova instância já está `Running` e o `MultipleInstances=IgnoreNew` a descarta;
se a nova instância não subir, o reinício por falha é a rede de segurança que a pausa jogava fora.

O instalador segura `Local\HangarInstall` e, na janela longa dele, pausa os reinícios automáticos
existentes por **re-registro do XML** (`Suspender-Recuperacao` remove o nó `<RestartOnFailure>`,
`Restaurar-Recuperacao` o repõe no `finally`). `Set-ScheduledTask` não serve nem recebendo um
`-Settings` novo sem recuperação: ele funde com o XML existente e o `<Interval>` sobrevive sozinho.
O re-registro preserva gatilhos, principal, ação, diretório e demais configurações, e **não**
derruba instância em execução (medido com a tarefa `Running` e o processo filho vivo antes e
depois). O restore é cirúrgico — repõe o nó no XML **atual**, porque entre a pausa e o `finally` o
passo 7/8 re-registra a tarefa com caminhos novos e reescrever o XML velho desfaria a instalação
que acabou de rodar; tarefa que já voltou com recuperação própria sai sem toque. A vigia usa a
mesma exclusão e também reconhece processos vivos de instalação/atualização. A exclusão por tarefa
serializa reinícios concorrentes; não há PID ou arquivo de manutenção permanente que possa ficar
preso após um crash.

Verificação no Linux com PowerShell 7: `scripts/test-windows-tasks.ps1` exercita seleção,
identidade, ordem de parada/início, recusa de duplicação, falhas, exclusão entre processos e
restauração após erro. A sonda HTTP foi executada contra servidor local com 200, 404, 500 e
conexão sem resposta.

Verificação no **Windows**, contra o Agendador de verdade: `scripts/test-windows-tasks-reais.ps1`
(pula fora sozinho onde o módulo `ScheduledTasks` não existe). Registra uma tarefa descartável com
a forma das reais, põe para rodar e prova pause/restore: recuperação desligada, definição inteira
preservada, instância viva sobrevivendo ao re-registro, restore cirúrgico depois de um 7/8
simulado e restore idempotente. Existe porque o teste simulado não reprova XML inválido — foi
assim que a pausa por `Set-ScheduledTask`, recusada em 100% das chamadas, chegou a produção. A
prova de UAC e de sobrevivência real do atualizador/psmux no Windows ainda é necessária; os testes
de processo continuam simulados.
Referência: [configurações nativas do Agendador](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtasksettingsset?view=windowsserver2025-ps).

## Instalação Windows mantém o nível de permissão

(12/09/2026). `install.ps1` aceita execução comum ou elevada. O processo escolhe `Limited` ou
`Highest` para backend, frontend quando necessário e vigia, sempre com `LogonType Interactive`.
O atualizador é filho do backend e herda sua permissão. O instalador confere nível e logon após
registrar; falha no modo elevado não pode reutilizar silenciosamente uma tarefa comum.

Os atalhos no Menu Iniciar e na Área de Trabalho usam `shell/build/icon.ico`, gerado do
`assets/brand/icon.svg` (com a versão de dois arcos em tamanhos pequenos), e o flag `RunAsUser` acompanha o nível da instalação.
`shell/build/icon.png` usa a mesma imagem na janela Electron e no empacotamento. A Área de
Trabalho vem de `GetFolderPath`, inclusive quando redirecionada. O Electron já aberto precisa
ser fechado pelo usuário para a próxima abertura assumir a elevação.

`Get-InstallRunLevel` consulta a tarefa do backend antes de alterar a instalação: se ela está
em `Highest`, uma chamada comum para com orientação para atualizar pelo app ou usar PowerShell
elevado. Isso preserva a escolha sem configuração adicional. No modo comum, firewall e Modo
Desenvolvedor continuam usando UAC pontual; o reparo de tarefas antigas com dono Administradores
permanece. A antiga proibição está em [superado.md](superado.md).

Verificação: `scripts/test-install-elevation.ps1` exercita os dois níveis, os quatro pontos de
registro com comandos simulados e os bytes de elevação do atalho. Executado com PowerShell 7 no
Linux; UAC, Agendador, atalhos e reinício precisam de validação na máquina Windows.
Referências: [principal das tarefas](https://learn.microsoft.com/en-us/powershell/module/scheduledtasks/new-scheduledtaskprincipal)
e [flags do atalho](https://learn.microsoft.com/en-us/openspecs/windows_protocols/ms-shllink/ae350202-3ba9-4790-9e9e-98935f4ee5af).

Outras correções medidas em 10/09: com Tailscale publicado o firewall nem é perguntado
(`serve` entrega em localhost, o firewall não vê porta); o HTTPS do tailnet
  desligado abre `login.tailscale.com/admin/dns` e ESPERA Enter pra tentar de novo (um usuário
  ficou parado ali sem saber onde ir); o symlink sem admin é conferido no 1/8 (`Symlink-Funciona`)
  e, faltando, o instalador liga o Modo Desenvolvedor por UAC ou abre `ms-settings:developers` e
  espera — antes a pessoa só descobria ao criar a primeira conta. Medido no 5.1: `Remove-Item`
  num symlink de pasta estoura `NullReferenceException`; apagar link é `[IO.Directory]::Delete`.
  **A sonda é `cmd /c mklink /D`, não `New-Item -ItemType SymbolicLink`** (medido 10/09/2026 na
  VM, PowerShell 5.1.26100, token restrito via `runas /trustlevel:0x20000`): com o Modo
  Desenvolvedor LIGADO o `New-Item` do 5.1 ainda falha com "requer privilégio de administrador",
  porque não pede o flag de criação sem privilégio; `mklink` e o `os.symlink` do Python (quem cria
  os atalhos das contas) funcionam. Com o `New-Item` o instalador dizia "modo desligado" pra quem
  tinha acabado de ligar. A sonda nova dá `False` com o modo desligado e `True` ligado.
  E `(Get-Command npm).Source` é `npm.ps1`: o `.vbs` da tarefa do front chamava
  `cmd /c "...\npm.ps1" run preview`, o cmd abria o `.ps1` no Notepad e a vigia repetia isso a
  cada 5 min — `-CommandType Application` pra resolver lançador.

## Instalador guiado: duas perguntas no passo 0, o resto é padrão

(`install.sh --avancado` /
  `install.ps1 -Avancado` devolvem o wizard; 09/09/2026, spec em
  `docs/superpowers/specs/2026-09-09-instalador-guiado-design.md`). Decisão do usuário para o
  time não técnico: token (continua pergunta — é o que se digita no celular quando o QR não
  dá) e "usar fora de casa?" (Tailscale instalado e logado no 1/8, publicado no 6/8 — o Linux
  passou a rodar `sudo tailscale serve` como o Windows já fazia). Quatro sabores de pergunta:
  `ask` sim por padrão, `ask_senha` sempre pergunta (sudo nunca aparece "do nada"), `ask_extra`
  não por padrão (persistência tmux, painel), e sem terminal tudo é NÃO — o `Pergunte` do
  Windows foi alinhado a isso (antes, sem console, respondia sim). Log em
  `~/.hangar/install.log` / `%LOCALAPPDATA%\hangar\install.log`, nunca no `--update` (o app
  lê `##HANGAR-AVISO##` da saída crua) e SEM o token: a URL de pareamento do `print_pairing`
  carrega o token, então QR e URL vão só pro `/dev/tty` e, no Windows, com o `Start-Transcript`
  pausado (ele captura `Read-Host` e `Write-Host`). O "pull automático" que parecia redundante
  com o Atualizar do app é o hook `post-merge`: complementares (o botão chama o mesmo
  `--update`); ele passou a instalar sem perguntar. `hangar-doctor` é UMA implementação
  (`app/doctor.py`), com cwd em `backend/` porque o `Settings` lê o `.env` pelo diretório
  atual; login do Claude é `EstadoLogin.loggedIn`, não `estado` (que só diz se o CLI
  respondeu); LAN é `estado == "ok"` do `alcance`. `--check`/`-SoChecar` delegam a ele.
  `-Update`/`--update` só baixa o dist do CI (sha diferente → o mais recente publicado, com
  aviso); nunca `npm ci`/build no update; sem download vira pendência `frontend` e o backend
  reinicia mesmo assim. Motivo medido na VM Windows: `.Content` do `.sha` vem como `byte[]` no
  PS 5.1 (`.Trim()` estourava) e o build local morre em `@rollup/rollup-win32-x64-msvc` ausente
  (npm ci com lock gerado no Linux). No Linux, `--check`/`-SoChecar` e `hangar-doctor` chamam
  `uv run --no-sync`; no Windows, `-SoChecar` (`install.ps1`) e o `hangar-doctor.cmd` chamam o
  `python.exe` do venv direto, sem `uv` — o mesmo efeito, nada é sincronizado.

## O bloco do MCP `hangar` no Codex é reconhecido pela tabela, não só pelos marcadores (30/09/2026)

`scripts/registrar-mcp.py` gravava `[mcp_servers.hangar]` entre `# >>> hangar: mcp` e
`# <<< hangar: mcp` e, ao rodar de novo, só procurava os marcadores: sem eles, anexava o bloco no
fim. Medido no Windows com o app desktop do Codex (26.928) logado: ao trocar modelo, ativar
plugin ou logar, o app reescreve o `config.toml` inteiro a partir do modelo interno dele — sem
comentários e com `http_headers` em subtabela (`[mcp_servers.hangar.http_headers]`). O servidor
continua lá, os marcadores não. No `-Update` seguinte o registrador anexou de novo e o arquivo
ficou com `[mcp_servers.hangar]` duas vezes: o Codex loga `Invalid configuration; using defaults
… duplicate key`, descarta o config inteiro, e o app cai em "Não foi possível carregar as
configurações da organização" — parecia rede, era parse. Aconteceu duas vezes no mesmo dia; na
primeira, o bloco em subtabelas foi tomado por "escrito à mão" e apagado, e o ciclo voltou.
Agora o registrador remove o bloco marcado, confere pelo `tomllib` se o app já gravou o servidor
com a mesma URL e o mesmo token (nada a fazer) e, senão, tira toda seção `[mcp_servers.hangar…]`
antes de anexar o bloco marcado — idempotente contra a reescrita do app e autocorretivo num
arquivo já duplicado. Teste em `scripts/test_registrar_mcp.py`.

## O instalador exige um agente de código, não o Claude Code

(06/10/2026) O `install.sh` e o `install.ps1` tinham o Claude Code como dependência obrigatória
do passo 1/8: sem ele a instalação parava em "faltam: Claude Code", mesmo para quem só usa Codex.
O Codex era só verificado e Pi, omp e Kimi nem isso. O pedido foi: o Claude continua o padrão,
mas não pode ser o único, e tem que haver pelo menos um agente.

Sem a opção, a escolha sai do disco e não vira pergunta nova, porque o guiado mantém as duas
perguntas: os agentes que já existem bastam, e só sem nenhum o Claude Code é instalado.
`--agentes=`/`-Agentes` e o `--avancado` escolhem explicitamente. O Claude segue pelo caminho
próprio (no Windows o `Instale-ClaudeCode` conserta o PATH que o instalador da Anthropic não
põe). Os outros rodam no 2/8, já com o venv, por `python -m app.harness_commands <cli>`: um
módulo só com biblioteca padrão para o instalador não carregar o backend inteiro, e que o painel
de Harnesses também lê, para o comando conferido de cada fornecedor morar num lugar só. Kimi no
Windows não tem comando conferido e vira pendência com o link. A prova do 2/8 é ter algum agente
no PATH; o `hangar-doctor` só dá erro quando não há nenhum.

## O `sudo` sem terminal: `sudo -S`, não `SUDO_ASKPASS`

(06/10/2026, bloco 1 do instalador gráfico.) O assistente do app nativo roda o `install.sh --app`
sem terminal e precisa entregar ao `sudo` a senha de administrador que a pessoa digitou na janela
do app. O caminho óbvio, `SUDO_ASKPASS` + `sudo -A`, não existe no `sudo-rs`, o `sudo` padrão do
Ubuntu 25.10. Medido em contêiner descartável (`docker run --rm`, sem terminal: `setsid`, stdin
nulo), com usuário comum no grupo de administradores e um auxiliar que imprime a senha:

| Medição | Ubuntu 25.10 (`sudo-rs 0.2.8`) | Fedora (`Sudo version 1.9.17p2`) |
|---|---|---|
| `sudo -A id -u`, com `SUDO_ASKPASS` | `invalid option provided`, rc 1 | o `-A` chama o `SUDO_ASKPASS`: `0`, rc 0 |
| `sudo id -u` (sem `-A`), com `SUDO_ASKPASS` | o `SUDO_ASKPASS` é ignorado: `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 | o `SUDO_ASKPASS` é ignorado: `sudo: a terminal is required to read the password; either use the -S option to read from standard input or configure an askpass helper` e `sudo: a password is required`, rc 1 |
| `aux \| sudo -S -p '' id -u`, senha certa | `0`, rc 0 | `0`, rc 0 |
| `aux \| sudo -S -p '' sh -c 'sudo id -u'` | o `sudo` de dentro não pede nada: `0` | o `sudo` de dentro não pede nada: `0` |
| `sudo -S`, senha errada | `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 | `Sorry, try again.`, `sudo: no password was provided` e `sudo: 1 incorrect password attempt`, rc 1 |
| `sudo -n true` logo depois de um `sudo -S` bem-sucedido, mesmo processo-pai | rc 0 | rc 0 |
| `aux \| sudo -S -p '' -v` e depois `sudo -n true` e `sudo -n <comando>`, sem terminal (direto, segunda chamada, em `$(...)`, em cano) | rc 0 nas quatro | rc 0 nas quatro |
| usuário fora do sudoers, senha certa dele | `sudo-rs: I'm sorry <usuário>. I'm afraid I can't do that`, rc 0 1 | `<usuário> is not in the sudoers file.`, rc 0 1 (antes dele, o aviso "We trust you have received the usual lecture…", que o `sudo` clássico imprime no primeiro uso de cada usuário) |
| auxiliar sai ≠ 0 (a pessoa cancelou) | `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 1 1 | `sudo: no password was provided` e `sudo: a password is required`, rc 1 1 |
| auxiliar ausente (caminho que não existe) | `bash: line 1: /nao/existe/askpass: No such file or directory` e as mesmas linhas da senha errada, rc 127 1 | `bash: line 1: /nao/existe/askpass: No such file or directory`, `sudo: no password was provided` e `sudo: a password is required`, rc 127 1 |
| `sudo -S -k -v -p ''` (conferência do app), senha certa / errada | rc 0 0 / `sudo: Authentication failed, try again.` (2×) e `sudo-rs: Maximum 3 incorrect authentication attempts`, rc 0 1 | rc 0 0 / `Sorry, try again.`, `sudo: no password was provided` e `sudo: 1 incorrect password attempt`, rc 0 1 |
| `apt-get install` de pacote que não existe, pelo `sudo -S` | `E: Unable to locate package pacote-que-nao-existe-hangar`, rc 0 100 | — |

No `sudo-rs`, auxiliar cancelado ou ausente sai com o mesmo texto da senha errada: só o código de
saída do auxiliar separa os casos.

Decisão: no `--app`, todo `sudo` do `install.sh` passa pela função `app_sudo`. Ela tenta `sudo -n`
(a credencial que o sistema já guardou) e, sem ela, só autentica: `"$HANGAR_ASKPASS" "<motivo>" |
sudo -S -p '' -v`. A senha só existe no cano entre os dois, nunca em variável, argv ou saída. Senha
recusada pede de novo com `"$HANGAR_ASKPASS" "<motivo>" --retry`, até 3 vezes. O comando roda
depois com `sudo -n <comando>`, sem a senha no stdin: com `sudo -S <comando>`, uma regra `NOPASSWD`
deixaria a senha na entrada do comando, e a saída dele se confundiria com a do `sudo`. Se o `-v`
passou e o `sudo -n` ainda pede senha (`timestamp_timeout=0`), a `app_sudo` para com a frase
`SUDO_NO_CACHE` e o código `sem-sudo`. O script da
Tailscale, que chama `sudo` por dentro, roda inteiro como administrador
(`sh -c 'curl … | sh'` pela `app_sudo`). A causa da falha sai do texto: fora do sudoers →
`sem-sudo`; auxiliar saiu ≠ 0 ou três senhas erradas → `senha-cancelada`; `apt` sem o pacote →
`pacotes-desatualizados`. Sem `HANGAR_ASKPASS` (o `--app` rodado à mão) a `app_sudo` nem chama o
`sudo -S`, e a falha fica sem código. Nunca `SUDO_ASKPASS`, `sudo -A` nem `pkexec` (pede a cada
comando e, sem agente de polkit rodando, a janela nem aparece).

## Modo `--app`: o assistente responde pelas telas

(06/10/2026, spec `docs/superpowers/specs/2026-10-06-instalador-grafico-design.md`, bloco 1.) O
assistente do app nativo instala pelos mesmos `install.sh`/`install.ps1`, sem terminal, lendo
marcas na saída. A regra "sem terminal, tudo é NÃO" continua valendo para CI, ssh e `curl | bash`;
o `--app`/`-App` é a exceção declarada, porque a pessoa já respondeu nas telas: `ask` sim,
`ask_senha` sim, `ask_extra` não, Tailscale por `--tailscale=sim|nao` (`nao` vale mesmo com ela
instalada). A senha de administrador vem da janela do app: no Linux todo `sudo` passa pela
`app_sudo`, que pede a senha ao auxiliar do app (`HANGAR_ASKPASS`) e a entrega ao `sudo -S` pelo
cano; no Windows o `Eleva-E-Roda` pede o UAC mesmo sem terminal. Nada espera
teclado: o login da Tailscale vira `##HANGAR-LINK## tailscale-login` e o HTTPS desligado vira a
pendência `tailscale-https`, e quem roda de novo é o app. A senha do celular chega por
`HANGAR_TOKEN` (nunca argv nem saída) e sai do ambiente antes de qualquer instalador de terceiro;
token já gravado é mantido. Ela recusa o que não é ASCII visível (acento), `#`, `$`, aspas, `\` e espaço nas pontas: o backend lê o
`.env` pelo python-dotenv, que corta o valor em ` #`, expande `${VAR}` e tira aspas, e a senha
gravada sem aspas não chegaria inteira. O `--app` não abre o app nem o navegador no fim, não
imprime o token, e no Windows escreve em UTF-8. O bootstrap puxa sem hooks (`core.hooksPath`
vazio): o post-merge rodaria um `--update` inteiro antes do `##HANGAR-PROTOCOLO##`. Ele também
tira `HANGAR_TOKEN`/`HANGAR_ASKPASS*` do ambiente e só os devolve na linha do instalador. No `install.ps1`, o portão do 1/8 só barra
pendência sem código: as que levam código (`Add-AppPending`) são extras, e o app as mostra no fim.

Marcas, uma por linha: `##HANGAR-PROTOCOLO## 1` (primeira), `##HANGAR-PASSO## <etapa> <estado>`,
`##HANGAR-ITEM## <id> <estado> <texto>`, `##HANGAR-LINK## tailscale-login <url>`,
`##HANGAR-ERRO## <código>` (logo antes da falha essencial, também fora do `--app`),
`##HANGAR-PENDENCIA## <código> <texto>` e `##HANGAR-FIM## ok|pendente|falhou` (última, de um
`trap EXIT` no bash e do `finally` no PowerShell). `##HANGAR-FALHA##` e `##HANGAR-AVISO##` seguem
como estão (`atualizar.py`). Mudou alguma marca: sobe o número do `##HANGAR-PROTOCOLO##` nos dois
instaladores e no app.

Sudo sem terminal, medido em [O `sudo` sem terminal: `sudo -S`, não `SUDO_ASKPASS`](#o-sudo-sem-terminal-sudo--s-não-sudo_askpass):
a senha entra pelo `sudo -S`, porque o `sudo-rs` do Ubuntu não aceita `-A` nem lê `SUDO_ASKPASS`, e
o script da Tailscale roda inteiro como administrador dentro de `sh -c`.

## Atualizar para no binário publicado (06/10/2026)

Na máquina do Waldir (Linux, canal `hangar-server-parte1`) o Atualizar foi ao topo `a810120b0`,
no contrato 36, com o binário mais novo publicado ainda no 35: o Supervisor recusou o
hangar-server e o Python atendeu sozinho ("o binário fala outro contrato interno"). O
`server.yml` publica minutos depois do push, e só quando `crates/` ou os caminhos dele mudam.

O manifesto já traz o `commit` do build, então a escolha não precisa de nada novo no publish:
o protocolo daquele commit é o do binário. A busca percorre só os commits de primeiro pai que
mexeram na linha do `RUST_SERVER_PROTOCOL` (`git log -m --first-parent -G`; `--diff-merges` só
existe do git 2.31 em diante), e o pai de cada um é o último com o número anterior. O aviso não
promete que o build vem: o mesmo estado aparece com o CI quebrado. Prova em clone descartável, 06/10: checkout em `d99d02e9` (34), release
`server-hangar-server-parte1` com o binário de `ce73e54f3` (34), topo `73727328e` (36): parou
em `5b103eede`, o último commit no 34, e baixou os dois binários com o sha256 conferido.

Build de UM sistema que falha: no manifesto antigo ele sai da lista e o asset velho fica sem
commit conhecido, então esse sistema vai ao topo, como antes. O manifesto por sistema
(`platforms.<sistema>` com `commit` e `protocol`) resolve: a escolha usa a entrada do próprio
sistema, e o `commit` do topo só vale quando a chave `platforms` não existe.

15/10/2026: os casos "sem como conferir" iam ao topo. Reproduzido em teste (checkout e binário no
35, topo no 36): com o `server-latest.json` respondendo 404, sem build deste sistema no
manifesto, sem rede ou com o commit do binário ausente, o checkout terminava no 36 com o binário
35. O 404 na main não é "sem release": o `gh release upload --clobber` do `server.yml` apaga o
manifesto antes de subir o novo, e cada sistema que publica regrava. Agora o contrato que vale
nesses casos é o do checkout: trocar dele sem binário conferido é o que deixa o Python sozinho.

**Commit exato, não o mesmo contrato (16/10/2026).** No notebook do Jefferson o Atualizar das 17:41
levou o checkout a `2970723f` enquanto o build do `server.yml` de `cb6daadbb`, que trazia a correção
da fila, ainda rodava (~30 min). Com o contrato igual, o código foi ao topo e o `hangar-server`
ficou o das 16:36: a correção do Rust não chegou, e depois, com o binário publicado às 17:49, a
tela respondia "Não há versão nova" porque contava só commits. Agora o checkout para antes do
primeiro commit que muda o que o `server.yml` compila (commit só de Python chega, porque nele o
workflow nem roda), e a tela compara o sha256 dos binários instalados com o manifesto (resposta
guardada 5 min; "Procurar" relê), oferecendo a troca do binário mesmo com o código em dia.

**Conteúdo do build, não mudanças no histórico (19/10/2026).** A main recebeu uma branch que
havia mesclado a main como segundo pai. O merge trazia o mesmo Rust do binário publicado, mas
o diff contra o primeiro pai parecia um build novo: a atualização parava em `991bb4f1`.
A escolha agora percorre candidatos do primeiro pai do mais novo para o mais antigo e usa
`git diff --quiet <publicado> <candidato> -- <paths>`. Teste com Git real cobre a branch
divergente, o merge da main dentro dela, o push como main, uma mudança Rust pendente e sua
reversão. A pendência também vai em campo explícito do estado, para os clientes não anunciarem
atualização completa quando o checkout ficou abaixo do topo.

**Reinício sem motor não deixa resultado antigo (19/10/2026).** Em reprodução isolada, o
lançador saiu antes de `app.atualizar` escrever: `reiniciar_agora()` devolvia o pid novo com
`ok: true`, mas `estado_para_tela()` conservava `pronto/ok: true` de outro pid. Os clientes
esperavam o pid novo até os dois minutos. O pedido agora publica `rodando` antes de lançar,
e o leitor usa o pid da trava para declarar a falha mesmo sem a primeira escrita do motor.
Um filho real que lança exceção antes do motor prova o desfecho e a preservação do traceback
no log privado. Outro teste prova que HTTP do mesmo pid não confirma um reinício.
O Windows usa o lançador do venv, que cria outro interpretador (`PC/venvlauncher.c` do CPython):
o pid do POST pode diferir do motor mesmo com o serviço reiniciado. A correlação por `request_id`
independe desses processos; teste com pid de lançador diferente comprova o campo compartilhado.
As tentativas relatadas às 07:49–07:57 não têm POST no journal desta máquina; a reprodução
comprova o defeito do lançamento, sem atribuir a ele aqueles cliques sem a resposta capturada.

## Verificação local antes do push (06/10/2026)

**Rápido por padrão (14/10/2026).** A rodada da bateria do CI virou espera de mais de 40 min por push.
Numa rodada real de outra sessão (commit `a2c14ee7a6`): preparo 441 s, rust 327 s, backend 813 s,
nativo 738 s, front 1337 s (vitest do frontend 296 s, do core 54 s), mobile 184 s, com o Linux
terminado em ~33 min; a VM recusou SSH e o processo ficou mais de 1 h no `WaitOne()` sem prazo, com a
fila da máquina parada atrás. O front e o mobile ainda entram no `flock` global dos scripts npm, que
é o das suítes de todas as sessões. O padrão passou a rodar só o ligado aos arquivos mudados, fora
desse `flock`; a bateria inteira fica no CI e no `--completo`.

Cada tarefa levava ~4 h, ~3 delas esperando CI. A trava `pre-push` só rodava os testes ligados aos
arquivos quando conseguia perguntar pelo `/dev/tty`; o comando de push de um agente nunca tem esse
canal, então os testes eram pulados e o primeiro lugar onde rodavam era o CI (Windows: 25–40 min
por rodada).

Medido nesta máquina (16 núcleos, `CARGO_BUILD_JOBS=4`, `nice`/`ionice`), commit `c14dffe8f`:

| Passo | Frio | Quente, sem mudança | CI (mediana do job) |
|---|---|---|---|
| rust: `cargo test --workspace` + contrato do cano | 280 s | 169 s | Server Linux 865 s |
| backend: pytest inteiro (+ contratos Rust) | 711 s | 680 s | backend 848 s |
| front: check, vitest, build | 382 s | 386 s | frontend 791 s + dist 66 s |
| nativo: `cargo test` do `desktop-native` | 475 s | 2 s | (o CI só compila release) |
| Rodada inteira (`--tudo`) | 1874 s | 1263 s | — |

Carga máxima (load 1 min) durante as rodadas: 6,3 a frio, 3,9 quente. Uma rodada de verdade só roda
os passos do que mudou, e árvore que já passou não roda nada.

- **Árvore fixa, não `CARGO_TARGET_DIR` compartilhado.** Target dividido entre checkouts rodou
  binário velho. Aqui o checkout é sempre o mesmo, e o `git checkout` muda a data só do que mudou,
  então o cargo recompila só isso. Prova: dois commits irmãos mudando uma string do
  `hangar-server`, verificados em sequência; o binário que o pytest usa trazia só a string do commit
  da vez (depois de B: `A=0 B=1`; de A de novo: `A=1 B=0`). Essa rodada, mudança de uma linha
  em `crates/`, levou 761 s: Rust 170 s, pytest 585 s.
- **Depuração só de linhas** (`CARGO_PROFILE_DEV_DEBUG=line-tables-only`). Cada arquivo de teste
  vira um binário; com depuração completa o target do `crates/` tinha 27 GB no Linux, e na VM
  encheu os 14 GB livres do C: no meio da compilação. Com só linhas: 11 GB. No Windows também sem
  incremental: o target lá fica em 5,4 GB e mora na VM entre rodadas. Mínimo de 4 GB livres para
  começar e um vigia que interrompe o passo abaixo de 2 GB.
- **Ambiente de runner**: HOME vazio, `TMUX_TMPDIR` e `XDG_RUNTIME_DIR` próprios, `CI=1`, só o PATH
  das ferramentas. O `omp` é o da versão fixada no `ci.yml`, baixado com sha256; sem ele 20 casos
  falham. A pasta temporária do pytest fica em `/var/tmp`, fora do HOME e fora do tmpfs.
- **Windows na VM roda só Rust e os 6 `test_runtime_*`**, como o `server.yml`. O pytest inteiro lá
  nem coleta (`fcntl`, `os.geteuid`), e testes com `tmux -L … kill-server` falariam com o psmux
  real da VM. O `python3` da VM é o atalho da Microsoft Store: o teste do cano lança `python3`, e
  sem um executável de verdade (venv fixo em `C:\hangar-verificacao\py3`) dava `10054`.
- **O que a VM não reproduz**: o relógio dela está em 1 ms (`NtQueryTimerResolution`, algum
  processo o ergue), e o padrão do Windows é 15,625 ms: teste de prazo que passa aqui pode falhar
  no runner; 4 núcleos dedicados contra 4 vCPU compartilhadas, Windows 11 26100
  contra Server 2022, e o cache frio de cada rodada do CI.
- **O que só o CI faz**: build release (LTO fat na `main`; `zigbuild` com glibc 2.28 no Linux), macOS, a
  publicação dos binários e do `dist-latest`, e o `passos` (o pre-commit faz o mesmo por commit).
- **Cache do Rust no Actions: só a `main` grava, toda branch lê o dela.** Em 07/10 o repositório
  estava em 7,3 GB de 10 GB, com cópias de 2 a 3,6 GB gravadas por branches `fix/*`; a parte1
  tinha perdido o cache de Linux e Windows. O Windows do `server.yml` levou 1503 s frio contra
  1144 s quente na mesma árvore (PRs de medição #95/#96).
- **Exigido só onde vira PR ou main (decisão do dono, 07/10).** Exigir em todo push travava quem só
  quer guardar trabalho numa branch. O que importa é o que vai ser revisto e juntado: o PR nasce
  verificado inteiro, e cada push seguinte nele ou na `main` também.
- **`release.yml` saiu.** Empacotava o shell Electron em tag `v*` (última em 25/08); nada no
  instalador, no Atualizar, no app nativo nem no site lê essas releases.

## Build e teste locais rápidos (23/10/2026)

Regra: otimizar sem tirar verificação; cada mudança provada com o mesmo resultado. Medido nesta
máquina (12 núcleos, 23 GB, outras sessões abertas), uma compilação ou suíte por vez, `-j 2`; pico =
maior RSS de um processo da medição.

- **Dev não acelera a troca.** `cargo build -p hangar-server`, target comum aquecido, uma linha
  mudada: release 29 s, dev 48 s (pico ~3,2 GB nos dois). Do zero: release 1.124 s, dev 416 s. Os
  10–13 min da troca em `~/.hangar/bin` eram de target frio, por isso o `cargo-pinned` (target comum)
  e não o perfil dev.
- **Target comum sem binário velho.** O cargo separa os crates do workspace por caminho do checkout;
  o que se misturava era o executável final, que todos gravam no mesmo lugar. O `cargo-pinned`
  copia o executável na mesma trava do build (`scripts/test-cargo-pinned.sh` prova que o build de
  outro checkout não muda a cópia).
- **Mensagens com carimbo.** `npm run i18n:compile` do front (core junto): 18–22 s e 670–830 MB
  antes; 0,7 s e 72 MB sem mudança em `messages/`. Saída igual à do `paraglide-js compile` direto
  (hash da árvore em `scripts/test-i18n-compile.mjs`).
- **Cache de transformação do Vitest** (`experimental.fsModuleCache`). Suíte do front: 382 s sem,
  348 s na primeira rodada com ele, 310 s com ele quente; os mesmos 2.300 testes passando, um a um.
  Core: 32 s → 25 s, os mesmos 1.531.
- **Seleção do front pelo core.** Arquivos do front escolhidos pelos nomes que o diff mudou: em
  commits recentes, `format.ts` 14 arquivos (76 pelo arquivo inteiro), `activity.ts` 11,
  `updateAll.ts` 12, contra 326 que importam o core. Antes, nenhum teste do front rodava.
- **Memória na verificação.** `verificar-local` rápido de um commit com uma linha no
  `hangar-server` e uma no front: as duas rodadas passaram, com 10 GB livres; a espera não disparou,
  então o ganho sob pouca memória não foi medido.
- **Não entrou, por mudar o resultado ou não render:** tipos das mensagens em `.d.ts` com `.js` sem
  JSDoc (suíte do front 382 s → 242 s, mas o `svelte-check` passa de 0 a 14.257 erros nos `.js`
  gerados, e compilar as mensagens vai de 15 s a 77 s); uma cópia só das mensagens (o runtime do
  front tem outra estratégia de idioma, e 20 testes do app trocam o idioma no runtime dele, que o
  core passaria a seguir); `tsc` incremental no core (7,8 s, 4,1 s, 7,7 s: ganho instável).

## Verificação local em paralelo (09/10/2026)

A rodada inteira levava ~27 min no Linux, com os passos em fila: rust 176–247 s, backend 718–755 s,
front 366–448 s, mobile 97–114 s, nativo até 150 s. Um passo instável custava a rodada inteira, porque
o registro só era gravado quando todos passavam, e com o Node 26 do sistema front e mobile falhavam
em massa (o jsdom fica sem `localStorage`).

Medido nesta máquina (24 núcleos, outras sessões trabalhando, load average até 29), `--tudo`, padrão
de 4 processos: preparo 24 s; rust 177 s, backend 180 s e nativo 12 s em faixas próprias; front 318 s
e mobile 139 s na mesma faixa; shell, statusline e pi 8 s. Caminho crítico ≈ 8 min.

- **pytest-xdist com `--dist loadfile`.** O backend sozinho: 755 s em fila; com xdist, 171 s com 4
  processos, 95–117 s com 8 a 16. Arquivo inteiro por processo porque há testes com nome fixo de
  sessão tmux (`cp-test-termsock`), que colidiam quando o mesmo arquivo se dividia.
- **A suíte dependia da ordem dos arquivos.** Em fila, na ordem alfabética do CI, passava; repartida,
  cada rodada quebrava um arquivo diferente, e cada um passava sozinho. Quatro origens: o Timer de
  confirmação de entrega do `app.api` (~8,5 s) sobrava de um teste e falava com o tmux no meio do
  arquivo seguinte (`test_api.py` → `test_terminal_observer.py`, e por tempo o
  `test_create_modelo_api.py`); e `test_codex_wrapper.py` tirava `app.adapters` do `sys.modules`
  sem devolver, e o pacote reimportado nascia sem o atributo `adapter`; e `test_sync.py` recarregava
  o `app.api` (`importlib.reload`), trocando o `app` global, e o `test_create_modelo_api.py` seguinte
  perdia o serviço de contas Codex que tinha montado; e, na tomada da porta pelo Python, os dois
  `uvicorn.Server` saíam fora de ordem e deixavam o tratador de `SIGTERM` de um servidor encerrado
  instalado, com o que o `sse-starlette` ligava o `should_exit` global e toda resposta SSE seguinte
  saía vazia (`No response returned` no `test_internal_side_events.py`). O conftest restaura os
  sinais e esse estado ao fim de cada teste. Detector usado: um plugin de
  pytest que lista as threads vivas ao fim de cada arquivo; o terceiro, por bissecção da ordem; o
  quarto, imprimindo o tratador de `SIGTERM` ao fim de cada teste.
- **Preparo único.** Com os passos em paralelo, um `cargo build` de uma faixa reescrevia binário que
  outra executava, e o pytest chegava antes das sondas de contas (~30 falhas falsas). Os passos não
  compilam mais nada no Linux.
- **Duas faixas, e seleção por quem importa (22/10/2026).** No notebook (12 núcleos, 23 GB), uma
  rodada rápida que mexia em `backend/app/transcript.py`, `packages/core/src/format.ts`, um crate de
  base e o nativo levou mais de 49 min, com load 21 e 13 GB em swap; a máquina ficou inutilizável.
  Com uma faixa por passo pesado, cargo test (12 threads lançando tmux e servidores), pytest com 4
  processos, svelte-check e vitest rodavam juntos: o `it` do `hangar-server` foi de 148–172 s para
  954 s, com 19 falhas de prazo por carga. A palavra solta `transcript` escolhia 63 arquivos de teste
  (14 o importam). O core é reexportado inteiro pelo `index.ts`, e `vitest related` sobre um arquivo
  dele puxou quase todos os testes do front (mais de 40 min). Agora: os passos de cargo dividem uma
  faixa e o resto outra; `RUST_TEST_THREADS=4`; o front só roda os testes dele com arquivo do front
  mudado (tipos seguem pelo `tsc`/`svelte-check`); o backend casa `app.x`, `from app import ... x` e a
  linha só com o nome num import em várias linhas.
- **Teto de tamanho no `target` da árvore fixa.** Ela reaproveitava a compilação (0 recompilações
  na mesma árvore), mas o `crates/target` só crescia: 52 GB em três dias, 20 GB de sessões
  incrementais e 32 GB de `deps` com hashes que nenhum build seguinte usa. Poda por idade não serve
  (tudo tinha menos de três dias, e o disco é `noatime`). Acima de `HANGAR_VERIFICAR_TARGET_GB`
  (padrão 40), o preparo limpa e a rodada compila do zero.
- **O vitest usa `--maxWorkers`** com os processos configurados: os `vitest.config` limitam a 2 forks
  para várias sessões não encherem a RAM, e aqui a verificação roda sozinha na fila.
- **Dois testes do `hangar-server` liam a configuração de quem roda:** o tmux isolado lia o
  `~/.tmux.conf` (`base-index 1` mudava o alvo `=test:0.0`), e um `core.hooksPath` global fazia o
  remoto do teste de vagas ignorar o `pre-receive`. Agora `-f /dev/null` e o `core.hooksPath` do
  próprio remoto.
- **Cache Rust do CI.** O cache do `server` no Windows tinha 5,5 GB; com o macOS (2,8 GB) e o do
  backend, o repositório chegava ao limite de 10 GB, e o GitHub despejava o do Linux: o PR #118 deu
  `Cache not found` no `server-Linux` e compilou do zero (5 min 25 s de testes, 7 min 46 s de
  release, 1119 s no job). Os jobs que guardam `crates/target` compilam sem incremental e com
  depuração só de linhas, e as chaves ganharam `-v2-`: o `save` só grava quando a chave exata não
  existe, e sem chave nova o cache enxuto nunca seria gravado.

## Build Rust e CI menores (09/10/2026)

Issue #142. Um executável por arquivo de `crates/hangar-server/tests/` (94), cada um com o servidor
inteiro; perfil `dev` com depuração completa para quem não passava a variável do CI; e o CI ligando
LTO fat com uma unidade de código em todo push.

Medido num Intel Core i7-13700K (16 núcleos, 24 threads), 32 GB de RAM, SSD NVMe PCIe 4.0 com
btrfs, Linux 6.18, Rust 1.98.1 e `CARGO_BUILD_JOBS=12`: `cargo test --workspace --no-run` do zero
em `crates/`, depois tocando `src/lib.rs` e um arquivo de teste e rodando
`cargo test -p hangar-server --no-run`:

| | Antes (perfil padrão) | Antes (`line-tables-only`) | Depois |
|---|---|---|---|
| Compilação do zero | 79 s | 58 s | 55 s |
| `target` | 42,7 GB | 15,6 GB | 4,3 GB |
| `debug/deps` | 35,9 GB | 11,9 GB | 2,0 GB |
| Executáveis acima de 100 MB | 95 | 41 | 4 |
| Tocar `src/lib.rs` | 68 s | 47 s | 7 s |
| Tocar um arquivo de teste | 1 s | 0 s | 3 s |
| `target` depois dos dois | 84 GB | 30 GB | 5,5 GB |

Testes listados: 1651 no `hangar-server` e 1739 no workspace antes; depois da junção os mesmos (os
testes novos das correções abaixo vieram depois da medição).

- **Seis executáveis à parte.** `workspace_unavailable` esvazia o `PATH`; `runtime_diagnostics` e
  `terminal_diagnostics` instalam o logger global e contam avisos exatos; `contract_local_policy`
  troca `HOME`, `USERPROFILE` e `TZ`; `costs_hook` (os três de custos) instala o gancho de pânico do
  servidor, que tirava a mensagem de falha de todo o `it` (duas falhas apareceram sem motivo antes
  da separação).
- **O limite do diário atravessava os arquivos.** `groups_routes` e `groups_legacy` esperavam o
  mesmo `rust.groups_restore_failed` para `s0`; no mesmo processo o segundo nunca chegava
  ("condição não chegou em 5 s"), porque o `warn_limit` é do processo. A chave do diário ganhou o
  destino (o Python do servidor): em produção há um só, e cada servidor de teste tem o seu. Um
  `DiagClient` próprio por limitador mudaria a produção, porque o cliente é criado em vários pontos.
- **Três intermitentes que a junção expôs no Windows**, corrigidos na causa:
  - `costs/origins` dava a pasta por igual quando a skill nova nascia no mesmo degrau do relógio do
    sistema de arquivos (1 a 16 ms no Windows) da mudança anterior. Data com menos de 2 s da
    varredura agora não vale como "não mudou".
  - `accounts/locks` fazia cada tentativa no pool de bloqueio. A tentativa sobrevivia ao
    cancelamento da espera e podia pegar a trava depois dele; agora roda na própria espera, já que
    o `try_lock` não bloqueia.
  - O `terminal_runtime` lia o arquivo de estado no meio da troca atômica do ator. No Windows 11, 286 de
    392 mil leituras feitas durante as trocas deram acesso negado. O leitor Python de
    `runtime_policy.py` (`session.patch_meta`) está exposto ao mesmo e ficou fora deste trabalho.
- **Um processo só para todos os testes exige higiene de processo.** No Linux, em 12 rodadas do `it`
  inteiro, três falharam por recursos de um teste que outro segurava:
  - **Filho de `fork` sem `exec`** (a sonda de `ptrace` do `uploads_store` e o filho parado do
    `accounts_catalog`) carregava cópias de todos os descritores do processo: o socket que outro teste
    fechou continuava aceitando conexão. O filho agora fecha o que herdou (`close_inherited_fds`).
  - **Porta "fechada" tirada de um listener solto** voltava a outro teste, que respondia no lugar. Agora
    é a porta 1 (`refused_address`), que o kernel nunca sorteia; o auxiliar confere uma vez que ela
    recusa e para com o motivo se algo escutar ali. Reservar com
    `bind` sem `listen` não serve: no macOS a conexão fica pendurada em vez de recusar.
  - **Executável gravado e rodado em seguida** podia dar "Text file busy" (ETXTBSY): um filho de outro
    teste, nascido enquanto o arquivo estava aberto para escrita, herdava o descritor. Quem grava agora
    é um `install` à parte (`write_executable` no `it`, `write_test_executable` nos testes de `src/`).
  - **"A porta fechou" logo depois do `stop`** espera a recusa com prazo (`assert_closed`), pela mesma
    janela entre o `fork` e o `exec`.
  - **Lease e portas reabertos logo depois de soltos**: um filho de outro teste, entre o `fork` e o
    `exec`, carrega por instantes a cópia do descritor (a trava do `flock` é da descrição aberta). O
    teste espera até 2 s (`lease_when_free` e o reinício do `plugin_loopback`). A produção tem a
    mesma janela, já que o servidor lança processos o tempo todo; trocar o `flock` pela trava do
    `fcntl`, que não passa ao filho, muda o contrato com o `WriterLease` do Python.
  - Os intermitentes de porta do `cano_v2` e do `plugin_loopback` e o do canal privado do `side`
    vieram do #140, com o mesmo conteúdo.
- **Dois que o pytest em 4 processos expôs no CI:** o `git commit` dispara `maintenance run --auto`
  em segundo plano, e o `maintenance.lock` sumia no meio da cópia do repositório do teste (a suíte
  desliga a manutenção automática); e o Codex deixava filhos em segundo plano (o clone dos plugins
  curados) escrevendo na pasta temporária do escritor de `config.toml`, e salvar as opções falhava
  ao apagá-la. No POSIX o Codex nasce num grupo de processos próprio (`_OwnGroup`), com o mesmo
  contrato do Job do Windows: prazo de saída, grupo encerrado e fim confirmado. Conserto no Python
  por decisão do dono: é defeito do que já existe, não recurso novo.
- **Intermitentes antigos do macOS, já presentes na `main`:** o `state::live` exigia nenhuma
  releitura logo depois de armar o observador, e o FSEvents entrega com atraso a criação das pastas
  do próprio teste; agora o teste espera o observador assentar. O `cargo test` do Server roda com
  `--no-fail-fast`: sem ele, o primeiro executável que falhava escondia os outros, e cada rodada do
  macOS mostrava um intermitente diferente.
- **Reexecução com nome curto passava sem testar nada.** No `it` o nome ganha o módulo; o filho com
  `--exact <nome>` rodava 0 testes e saía 0. Prova: com `assert_ran_one` e o nome antigo, os cinco
  falharam com `running 0 tests`.
- **Features iguais entre `-p` e `--workspace`.** `alloc` no `serde`/`serde_json` (vindo do
  `schemars` de teste do `hangar-codex`) e `signal` no `tokio` (do `hangar-cano`) faziam
  `cargo test -p hangar-server` e `--workspace` compilarem o servidor duas vezes, com uma cópia de
  cada. O `resolver.feature-unification` resolveria, mas no cargo 1.98 ainda pede `-Z`.
- **O Native não era lento por cache despejado.** Na `main` (run 37998405378) o cache foi achado nos
  três sistemas e o release levou 869 s (Linux), 1130 s (Windows) e 1392 s (macOS): o LTO fat com
  uma unidade de código, mais o GPUI vendorizado recompilando a cada checkout, porque o cargo julga
  crate de caminho pela data dos arquivos. Os 74 crates do job Linux do Server eram o
  `cargo install cargo-zigbuild`, não o release.
- **Com LTO as dependências saem em bitcode**, e o cache da `main` (`dist`) não serve ao `release`
  da branch. Por isso a chave leva o perfil, e o job `cache-release` da `main` grava o cache do
  `release` só quando as dependências mudam. O perfil nomeado no `Cargo.toml`, e não variável de
  ambiente, deixa os dois perfis em pastas próprias e sem valor a espelhar entre os jobs.
- **Cache do Actions em 10,98 GB** (limite 10 GB) antes: `server-v2` 2,5/2,2/1,6 GB, `native-*`
  0,8–0,9 GB, `backend-rust` 0,6 GB e 0,8 GB de uma chave sem `v2` que nenhum workflow usava.

## CI do Rust: publicar só na main e no canal (05/10/2026)

Medido em 05/10/2026, duração de cada job no GitHub (`gh run view --json jobs`), duas rodadas verdes
de cada lado. Antes, todo push em qualquer branch compilava release com LTO fat nos três sistemas e
publicava `native-<branch>`/`server-<branch>`:

| Job | Antes (2 rodadas) | Depois, branch comum |
|---|---|---|
| Native Linux | 858 s · 751 s (release) | `check` 868 s frio · 277 s com cache |
| Native Windows | 1077 s · 1357 s | não roda |
| Native macOS | 1197 s · 1410 s | não roda |
| Server Linux | 490 s · 561 s (teste + release) | `test` 262 s · 212 s |
| Server Windows | 857 s · 1029 s | `test` 504 s · 486 s |
| Server macOS | 872 s · 422 s | não roda (Server: só os testes) |

- **A regra saiu sem querer no mesmo dia.** O `cd1e36ca2` (`Revert "Merge ci/faster-builds…"`,
  sem motivo na mensagem) entrou pela branch `hangar-server-parte1` e não foi pedido. A otimização
  de 09/10 (LTO só na `main`, cache enxuto) foi escrita em cima do CI já revertido, e as branches
  voltaram a compilar e publicar pré-release a cada push. Em 10/10 a regra voltou sobre o CI de
  09/10: o build por sistema com publicação no próprio job ficou, e os passos de release, contrato
  do cano e publicação passaram a depender de `COMPILA`/`PUBLICA`. O canal virou `beta`; o
  `hangar-server-parte1` já tinha sido mesclado.
- **O `check` do nativo usa `cargo build`, não `cargo check`.** O perfil dev otimiza as dependências
  (`opt-level = 3`). O `check` as refazia só em metadados, 373 s a frio, além do `test`. O `build`
  confere o mesmo binário e deixa pronto o que o `test` usa: o `test` caiu de 525 s para 28 s.
- **Numa branch comum o job do Server se chama `testes`, não `build`.** Ele só roda o `cargo test` e
  o pytest do runtime; com o nome `build`, quem olhava um PR achava que o build de release tinha
  voltado (11/10/2026). O id do job continua `build`, e o nome segue a mesma condição do `COMPILA`.
- **O contrato do cano só roda onde há binário de release** (main, canal, `workflow_dispatch`).
  Numa branch comum ele não roda; quem mexe no `cano.py` confere com o `workflow_dispatch`.
- **Releases de branch antigas continuam publicadas.** Uma branch que parou de publicar e já tinha
  `native-<branch>`/`server-<branch>` segue servindo a última versão a quem estiver nela, porque o
  `rust_release.py` só recua para a main quando recebe 404.


## CI: testes em paralelo e cancelamento do run substituído (19/10/2026)

Medido nos runs de 19/10 (GitHub Actions, runners comuns de 4 vCPU; macOS de 3): Server levava
25–63 min e Native 5–52 min por push na main. O tempo ia em três lugares. O passo `dist` (LTO
completo, uma unidade de código) leva 11–36 min por sistema mesmo com o cache restaurado, porque só o
crate do app recompila e o link otimizado é sequencial. No `server.yml` os testes (`cargo test
--workspace`, 8–20 min) rodavam antes do build no mesmo job, então o job durava a soma. E o grupo de
concorrência da main não cancelava: cada push esperava o run anterior inteiro, com 20–28 min de fila
medidos. O cache do repositório estava em 9,8–10,6 GB do teto de 10 GB, com duas cópias do
`native-check-Linux` (chave nova não substitui a velha).

Mudou: testes num job paralelo ao build no `server.yml` (o `publish` por sistema exige o artefato dos
testes, então teste vermelho ainda segura a publicação daquele sistema); concorrência por job na main
e no canal, com o `publish` num grupo sem cancelamento; quem grava cache apaga as cópias antigas do
mesmo prefixo; o `check` do Native ganhou o mesmo `touch` do vendor que o `build` já fazia com a chave
exata (a chave inclui o hash do vendor, então a data antiga só é usada com o vendor igual ao
compilado; a data do commit não serve, porque um commit pode ser mais velho que o cache e ainda assim
trazer mudança). O LTO completo ficou: o Jefferson preferiu manter o binário como está.
Consequência: o publish do Linux no Server passa a esperar os builds e testes dos três sistemas
(`needs` de matriz), em vez de sair assim que o Linux terminava.

## Build pendente no CI não é aviso (19/10/2026)

Decisão do Jefferson: quando o commit mais novo ainda não tem o binário do servidor Rust publicado
para o sistema, a atualização vai até o último commit que tem e isso é uma atualização completa. A
pessoa que atualiza não tem como consertar um build pendente ou vermelho no CI; mostrar "Atualização
incompleta: … aguardando o build do servidor Rust" a cada clique era um erro sem saída. Medido no
mesmo dia: a DELPHI-02 ficou em `f086898f` enquanto dois pushes seguidos tiveram testes instáveis de
Windows (`artifacts_truncation…`, `claude_login`) segurando a publicação, e o aviso aparecia nas três
telas. Ao mesmo tempo, o botão de cima do nativo atualizava só o app: a release do `Native` sai antes
do binário do `Server`, o servidor "atualizava" até o commit que já tinha e o app novo instalava
sozinho. Agora o app novo espera o build do servidor desta máquina (`plan()` em
`desktop-native/src/update.rs`), e o aviso, o título "incompleta" e o estado `Pending`/`partial`
saíram do nativo, do PWA e do app; o diário guarda `atualizacao.limitada_ao_binario` como registro
normal.

## Atualização com contrato de API (19/10/2026)

Pedido do Jefferson: o botão Atualizar só aparece quando um clique conclui, atualiza servidor e app na
ordem certa sem perder nada no meio, e não compila a tela na máquina de quem atualiza. Medido no mesmo
dia:

- **O botão de cima nunca levava o servidor.** O app decidia "servidor desatualizado" pela contagem de
  commits, sem saber se o app novo precisava do servidor novo; o `Native` publica antes do `Server`, e o
  clique instalava só o app. A espera pelo build do servidor desta máquina (entrada anterior) segurava
  também o app que não precisava de servidor novo; saiu em favor do `CLIENT_API`.
- **O passo 4 ("Instalando dependências") levava ~2 min 15 s no Windows** só no `vite build` local,
  sempre que a atualização parava antes do topo (DELPHI-02, 15:41). Chegando ao topo, a atualização
  inteira leva 6–17 s (notebook e DELPHI-02). O dist publicado por commit não casava com o commit em
  que a atualização parava (o CI só compila a ponta do push), daí a chave pelas fontes.
- **O app perdia o servidor no meio.** O acompanhamento desistia em 10 min (300 × 2 s) e não era
  retomado depois de reabrir; casar a rodada pelo pid falharia no Windows, onde o lançador do venv
  muda o pid.
- **CI no mesmo período:** `Server` 25–63 min e `Native` 5–52 min por push, com 20–28 min só de fila.
- **Entrada presa no registro do Rust** (fechamento que falhava depois de parar o ator) vinha em laço
  desde 17/10 (`hangar` 1032×, `cx-herda4` 597×) e derrubava a troca de conta no limite com
  `relaunch_initialize`; corrigida no mesmo trabalho.
