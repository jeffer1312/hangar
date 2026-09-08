# Integração do Codex no Hangar

O Hangar mantém no Codex o ferramental compatível instalado no Claude Code desta máquina.
O importador oficial do Codex decide como converter plugins, hooks, comandos, subagentes e
configurações MCP. O Hangar organiza os gatilhos, acompanha a conclusão, trata compatibilidades
pontuais e registra o que passou a gerenciar. A integração funciona sem o Codex Desktop aberto.

## Responsabilidades e gatilhos

| Componente | Responsabilidade |
| --- | --- |
| Codex CLI | Executa a importação pelo app-server e instala/atualiza plugins e marketplaces pelos comandos nativos. |
| Backend do Hangar | Acompanha as fontes locais, agenda reconciliações e atualizações e publica o estado para o painel. |
| `hangar-codex-tui` | Aguarda a reconciliação antes de abrir o app-server da sessão e a TUI; informa falhas sem impedir a abertura. |
| Codex Desktop | Pode executar sua própria sincronização opcional, independente do Hangar. |

O reconciliador é `backend/app/codex_integracao.py`, e quem o chama é sempre o backend, em dois
momentos: quando o lançador abre uma sessão Codex (`POST /api/harness/codex/integracao/sessao`)
e no botão **Reconciliar agora**, em **Configurações → Harnesses → Codex**. Não há laço nem
rodada na subida do backend: o Codex converte, o backend decide quando, o lançador só avisa.

A abertura de sessão é um cache por conteúdo: o `estado.json` guarda a assinatura das pastas do
Claude da última rodada; assinatura igual, marketplaces dentro do prazo e última rodada sem falha
significam que o Codex nem é chamado. O intervalo de atualização dos marketplaces Git gerenciados
é de **6 horas**. Uma rodada parcial ou com erro só é refeita 5 minutos depois, na abertura
seguinte, e permanece visível até ser resolvida. O botão manual força uma nova tentativa de
atualização. Marketplaces locais permanecem locais e não recebem uma URL Git inventada pelo Hangar.

Na instalação inspecionada em 06/09/2026, com CLI 0.153.4, o bundle do Codex Desktop continha a
opção `external-agent-import-sync-enabled` e uma rotina própria de sincronização de 12 horas.
Esse é um comportamento observado daquela instalação, sujeito a mudanças no Desktop. O
intervalo de 6 horas do Hangar não depende dessa opção, e o Hangar não a liga nem a desliga.
Executar o CLI sozinho não reconcilia nada: quem reconcilia é o backend, e o lançador da TUI
só o avisa ao abrir a sessão — com o backend fora do ar, a sessão abre com o que já está lá.

O gatilho automático (abertura de sessão) obedece a três portões, e o botão
**Reconciliar agora** a nenhum deles: o interruptor **Sincronização automática** do card do Codex
(`codex_sync` no `runtime-config.json`, ligado por padrão), o kill-switch geral de automações
(`automations`) e `CP_CODEX_SYNC_ENABLED=0`, o desligamento duro por ambiente. Nenhum deles é uma
preferência escrita no Codex. A suíte de testes usa a variável para não acionar a integração no
perfil de quem executa os testes; os testes específicos instanciam o serviço com diretórios
temporários. O lançador da TUI espera a reconciliação por até 20 segundos e abre a sessão sem ela
quando o prazo estoura, avisando no pane.

Os hooks do próprio Hangar (`backend/hooks/`) não passam pelo importador: o hook de estado do
Codex é instalado por `backend/app/codex_hook_installer.py` na subida do backend, como no Claude
e no Kimi, e os demais (prévia, AskUserQuestion, pareamento) são exclusivos do Claude. Numa
máquina onde o instalador antigo escreveu o `hooks.json`, a primeira reconciliação remove
exatamente as entradas registradas no espelho `~/.codex/.hangar-hooks.json` (com backup) e o
apaga; sem isso cada hook passava a rodar duas vezes.

## Importação e compatibilidades

`backend/app/codex_importador.py` inicia um processo temporário `codex app-server --stdio`,
inicializa o protocolo e chama `externalAgentConfig/detect` e `externalAgentConfig/import`.
A resposta inicial com `importId` não significa conclusão: o cliente espera
`externalAgentConfig/import/completed` com o mesmo identificador e examina os resultados.
O processo temporário é encerrado ao finalizar ou cancelar a operação. Nenhum turno de agente
é criado para importar configuração.

As operações de plugins usam `codex plugin add <id> --json` e
`codex plugin marketplace upgrade <nome> --json`. A identidade e a procedência vêm do
importador e do cadastro nativo; o Hangar não deduz repositórios a partir do nome de um plugin.
Os plugins habilitados no Claude são a fonte do conjunto gerenciado. Um plugin que sai desse
conjunto só é desabilitado pelo Hangar se já havia sido adotado no registro da integração.
Plugins exclusivos do Codex ficam fora dessa administração.

Um marketplace pode declarar nomes diferentes para Claude Code e Codex, como
`thedotmack` e `claude-mem-local`. O reconciliador associa uma instalação nativa pelo nome do
plugin e pela origem confirmada do marketplace. O registro mantém o identificador da fonte e
o `id_codex` de destino; instalação, atualização, habilitação e desabilitação usam esse destino.
O painel mostra a identidade nativa. Homônimos de outra origem e associações ambíguas são
preservados com erro, sem escolher um plugin arbitrariamente. A ponte continua reconhecendo as
skills pela origem Claude e só retira duplicatas quando o destino nativo está habilitado.

Script de `~/.claude/hooks/` que é symlink não é copiado pelo importador nativo; depois da
importação o Hangar cria em `~/.codex/hooks/` um link para o arquivo real (`codex_hooks_arquivos`),
sem cópia. Sem equivalente no Claude, vira aviso nomeando o arquivo.

O `security-guidance` emite telemetria (`metrics`, `rewakeSummary`) e, no bootstrap, duas linhas
JSON com anúncio `async`. O adaptador `codex-hook-json.py`, instalado em
`CODEX_HOME/.hangar-hooks/`, converte somente esse plugin para o contrato estrito do Codex.
Mensagens, contexto, decisões de bloqueio, stderr e código de saída são preservados. A mudança
dos comandos exige nova aprovação na interface de hooks do Codex; o Hangar não concede confiança.

Hooks, comandos, subagentes e MCPs passam por uma área temporária com apenas as fontes da
importação. Os caminhos de saída são remapeados para os destinos definitivos. O escritor
oficial `config/batchWrite` edita uma cópia de `config.toml`; o Hangar valida novamente o
conteúdo original antes de substituir o arquivo vivo. Isso mantém a escrita de TOML sob a
responsabilidade do Codex e permite detectar alterações concorrentes.

O bloco global `settings.env` participa da categoria nativa `CONFIG`. A área temporária recebe
somente `env` e `hooks` do `settings.json`, e o reconciliador mescla exclusivamente as variáveis
convertidas em `shell_environment_policy.set`. Não transfere `inherit`, filtros de ambiente,
modelo ou permissões produzidos nessa conversão. Variáveis iguais às já importadas são adotadas;
depois disso, alterações na fonte atualizam as variáveis gerenciadas. Remoções só retiram valores
gerenciados que ainda correspondem ao manifesto. Colisões diferentes e sem proveniência são
preservadas com aviso. Fonte inválida ou conversão incompleta não equivale a remoção.

Essas variáveis podem conter credenciais de serviços usados pelos plugins. Seus valores ficam
nos arquivos locais de configuração, manifesto e backups restritos, sem aparecer no painel ou
nos diagnósticos públicos e sem serem versionados. As sessões seguintes recebem a configuração
atualizada; o contexto de processos já abertos não é reescrito.

As compatibilidades próprias do Hangar ficam em `backend/app/codex_compat.py` e `codex_instrucoes.py`:

- `CLAUDE.md` tem prioridade sobre `AGENTS.md`, com `CLAUDE.MD` como segunda opção. O Hangar
  cria `AGENTS.override.md` como link para a fonte, no Codex home (global) e nos escopos dos
  projetos registrados no `config.toml`. O lançador prepara também o cwd novo, antes do
  app-server. O conteúdo é carregado pelo Codex, sem ordem de leitura nem hook de contexto.
  `AGENTS.md` permanece intacto; um override pessoal preexistente é preservado com erro explícito.
  A ordem de leitura gerenciada antiga é removida do `AGENTS.md` global, com backup.
- Os aliases são locais à instalação e não devem ser commitados: o Hangar os grava no
  `.git/info/exclude` do repositório, então não aparecem no `git status`. Para abrir pelo IDE/CLI cru
  um projeto ainda não registrado, rode **Reconciliar agora** após registrá-lo no Codex, ou
  abra-o primeiro pelo Hangar. Sem preparação, vale a precedência padrão do Codex.
  Sistemas sem permissão para links usam cópias verificadas, atualizadas na reconciliação e
  na abertura pelo Hangar; com links, edições da fonte são vistas na próxima sessão diretamente.
- O limite nativo de instruções passa a pelo menos 1 MiB e cresce com as fontes conhecidas,
  preservando um limite maior já configurado. O lançador também calcula o limite antes de abrir
  o app-server. Sessões já abertas não recebem um novo contexto inicial. Os fallbacks
  `CLAUDE.md`/`CLAUDE.MD` continuam configurados para projetos ainda sem alias.
- Hooks `SessionEnd` com timeout numérico acima de 3 segundos são limitados a 3 segundos.
- O hook RTK reconhecido passa pelo executor `scripts/codex-hook-allow.py`, que preserva a
  execução e seu código de saída e completa `permissionDecision: allow` quando a reescrita
  `updatedInput` exige isso. Não existe autorização genérica de hooks nem reescrita arbitrária
  de comandos de shell.

Skills pessoais e conteúdos ainda não cobertos pela instalação nativa podem usar uma ponte de
compatibilidade. Ela evita duplicar skills já fornecidas pelos plugins nativos gerenciados, e
não linka o que o Codex já lê sozinho em `~/.agents/skills`. Essa pasta é fonte compartilhada
(Pi, Kimi e omp): a ponte só retira dali uma cópia que o próprio Hangar registrou, e só quando o
plugin nativo correspondente está confirmado; cópia pessoal idêntica ou divergente fica, com aviso.
O reconciliador reaproveita a descoberta de fontes da antiga `skill_bridge.py`, mas é o único
caminho do Hangar responsável pelos destinos Codex dessa integração. No Windows, uma cópia
registrada por arquivo serve de alternativa quando não é possível criar o symlink.

## Migração, preservação e backups

O primeiro ciclo adota a integração existente de forma controlada. A antiga ponte geral deixa
de incluir o Codex em `TARGETS`, e `scripts/install-skills-bridge.sh` deixa de escrever sua
persona, hooks e skills. As pontes e os pacotes do Pi e Kimi continuam pelos caminhos anteriores;
o omp continua usando sua descoberta nativa.

O reconciliador mantém o manifesto em:

```text
~/.hangar/codex-integracao/<identidade-do-codex-home>/
  estado.json
  backups/
```

O destino padrão é `~/.codex`; `CODEX_HOME` pode selecionar outro diretório. A identidade do
registro deriva do caminho resolvido desse destino, evitando misturar instalações distintas.
O lock do sistema operacional fica em `CODEX_HOME/.hangar-integracao.lock`, fora da pasta do
manifesto. Ele serializa reconciliações do Hangar em processos diferentes que compartilham
o mesmo Codex Home, mesmo quando esses processos usam valores diferentes de `HOME`.
No mesmo serviço, chamadas simultâneas ao botão compartilham a tarefa em andamento.

O manifesto distingue conteúdo gerenciado de conteúdo exclusivo do usuário. Colisões sem
procedência conhecida devem ser preservadas e informadas. Nos itens adotados, o Claude é a
fonte da configuração sincronizada; alterações manuais no mesmo item não constituem uma
segunda fonte. Remoção e desabilitação ficam restritas ao que o registro identifica como
gerenciado. Arquivos reais e links particulares fora das fontes reconhecidas da ponte não
devem ser apagados como se fossem symlinks antigos do Hangar.

Antes de substituir conteúdo existente, a camada de escrita guarda seu backup. O backup é
identificado pelo caminho original, mantém os bytes anteriores e a informação do symlink
quando aplicável, e preserva a primeira cópia daquele caminho em vez de sobrescrevê-la em
cada execução. A pasta é criada com permissão restrita e os arquivos de backup com modo
`0600` em plataformas que aplicam permissões POSIX. Configurações MCP podem conter valores
de ambiente sensíveis; o conteúdo dos backups não deve ser publicado junto de um diagnóstico.

A troca é feita por arquivo temporário e substituição atômica, com comparação do conteúdo
anterior perto da escrita. Mudanças concorrentes fazem a etapa reler e tentar novamente,
com limite de tentativas. O lock coordena os processos do Hangar; o Desktop e editores
externos não participam dele. Não há uma transação global envolvendo todos esses programas.

Não existe botão de restauração automática dos backups. Para uma recuperação manual, pause
os gatilhos, identifique no backup o caminho e o conteúdo desejados, confira as alterações
posteriores e restaure apenas os itens necessários. Uma reconciliação futura volta a aplicar
a fonte Claude aos itens que continuarem gerenciados.

## Confiança, falhas e painel

A integração **não aprova hooks automaticamente** e não escreve decisões de confiança para
contornar a revisão do Codex. Uma normalização pode mudar o conteúdo de um hook e invalidar
sua aprovação anterior. O serviço consulta `hooks/list` quando disponível e informa a
pendência no painel e no lançador. Se essa consulta não for suportada pela versão instalada,
o painel mostra um aviso. A confirmação continua sendo feita pela pessoa no Codex.

As rotas autenticadas são:

| Operação | Resultado |
| --- | --- |
| `GET /api/harness/codex/integracao` | Snapshot de estado; não inicia processo, importação nem consulta de rede. |
| `POST /api/harness/codex/integracao` | Inicia ou compartilha uma reconciliação manual, retorna HTTP 202 com o snapshot. |

O snapshot informa `estado`, `etapa`, `ultima_execucao`, `proxima_atualizacao`, `plugins`,
`skills` (`{ponte, nativas}`), `erros`, `avisos`, `confianca_pendente` e `automatica`. `etapa`,
`erros` e `avisos` são mensagens `{codigo, params, texto}`: o catálogo é
`backend/app/codex_msgs.py` e a interface traduz por `harness_codex_m_<codigo>`; `texto` é o
fallback em português para um código que o app ainda não conhece. Os estados são `ocioso`, `executando`, `ok`, `parcial`,
`erro` e `indisponivel`. Datas usam ISO 8601 em UTC; a interface apresenta o horário local.

O card acompanha uma operação em execução aproximadamente a cada 1,5 segundo e encerra as
consultas quando ela termina. Trocar de servidor ou desmontar a tela aborta a consulta e
descarta respostas atrasadas. Uma falha de conexão permite tentar novamente. A consulta da
integração é independente da lista dos demais harnesses, portanto uma falha do Codex não
impede que os outros cards sejam exibidos.

Falhas de plugin ou marketplace são informadas por item e podem produzir resultado parcial.
CLI ausente ou Claude sem configuração tornam a integração indisponível. Não há promessa de
rollback global: etapas já concluídas podem permanecer aplicadas, com registro e backups;
as não concluídas são retomadas por uma nova tentativa. A falha da integração não impede a
abertura da TUI nem deve derrubar o restante do backend.

## Exclusões e limites

A integração de ferramental não migra sessões, histórico, credenciais de login/OAuth do agente,
modelo padrão, esforço de raciocínio, sandbox ou política de aprovação. O login compartilhado
e a propagação de credenciais já existentes no Hangar continuam em seus próprios serviços.
Isso não exclui os valores necessários à configuração de um MCP importado ou de `settings.env`,
incluindo tokens das ferramentas utilizadas pelos plugins.

O que o importador aceita depende da versão do CLI. Um hook, agente ou comando do Claude não
ganha compatibilidade semântica universal por ter sido convertido: dependências locais,
executáveis, variáveis de ambiente e permissões continuam necessários. Conteúdo exclusivo de
projetos não é inventariado como configuração global por este fluxo. A sincronização também
não envia retroativamente novas instruções para o contexto de uma conversa já em andamento.

Foram escritos testes do protocolo, das transformações, das rotas, dos gatilhos e do painel
com ambientes isolados. A interface foi conferida em navegador com dados simulados, em
larguras desktop e celular.

Em 06/09/2026, turnos reais do CLI 0.153.4 foram executados em Linux e Windows com
`exec --ephemeral --skip-git-repo-check --json`, pedindo somente a resposta `OK` e nenhuma
ferramenta. Ambos terminaram com código zero, responderam exatamente `OK` e produziram zero
eventos de ferramentas: 6,08 segundos no Linux e 6,82 segundos no Windows. Cada execução usou
`HOME` e `CODEX_HOME` temporários, com apenas uma cópia autorizada de `auth.json`; a remoção
da cópia de autenticação e dos diretórios temporários foi confirmada nos dois ambientes.

No Windows, a validação foi pelo CLI puro. O Codex Desktop do usuário não foi alterado nem
exercitado. Esses turnos comprovam a abertura e a resposta do CLI nesses ambientes; **não
comprovam a execução dos plugins e hooks importados**, pois não houve uso de ferramentas.
Passar nos testes e concluir uma importação também não comprova, por si só, que cada plugin
foi executado com sucesso dentro de uma conversa real.


A validação da implementação cobriu instalação vazia e anterior, atualização e desabilitação
sem apagar cache, plugins exclusivos, marketplace homônimo com origem diferente, imports
externos, repetição sem reescrita dos arquivos de configuração, cancelamento e fontes inválidas.
Na VM Windows também foram exercitados o protocolo stdio real, os comandos oficiais de plugins,
a normalização de caminhos estendidos/UNC e a alternativa de cópia gerenciada para skills.

A suíte completa do backend passou no Linux com **3.615 testes aprovados e 7 pulados** em
ambiente isolado de tmux/zsh. O teste adicional de migração da persona antiga também passou
nos dois sistemas. A checagem de tipos do frontend terminou sem erros e o build passou;
o teste do painel executou 8 casos. O CodeRabbit foi invocado uma única vez, mas não produziu
review porque o CLI estava sem autenticação e exigiu login em terminal controlado pelo usuário.
