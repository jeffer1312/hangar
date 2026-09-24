# Superado — mecanismos que não existem mais

Estes trechos descrevem como algo **funcionava antes**. Ficam registrados porque a
medição continua valendo como história, mas não descrevem o código de hoje: cada um
aponta a decisão que o substituiu. Não leia daqui para decidir implementação.

## Aviso de reinício do atualizador às sessões

(`atualizar._avisar_sessoes`, 25/08/2026 → removido em 24/09/2026). Antes de reiniciar, o botão
Atualizar rodava `hangar-send --group "[hangar] o backend vai reiniciar…"` para "as sessões vivas".
O atualizador não é uma sessão: o `hangar-send` caía na sessão do cliente tmux anexado. Com ela
fora de grupo o log mostrava `404 erro_sessao_sem_grupo`; com ela num grupo, o aviso ia só para
aquele grupo, assinado como se ela tivesse mandado. Nunca chegou a todas as sessões. As sessões
seguem vivas pelo reinício e o app reconecta sozinho, então o aviso saiu em vez de virar um
broadcast que custaria uma resposta de cada sessão por atualização.

## Login do ChatGPT (Codex) é UM login pra três CLIs, e quem faz é o app

(`app/oauth_codex.py` +
  a linha "Conta do ChatGPT (Codex)" do `NovaCredencialSheet` + `app/harness_saude.py`, 04/09/2026).
  Codex CLI, Pi e omp usam o MESMO OAuth — `client_id app_EMoamEEZ73f0CkXaXp7hrann`, mesmo
  `auth.openai.com/oauth/token`, mesmo fluxo de código de dispositivo — e cada um guarda o resultado
  no formato dele: `~/.codex/auth.json` (`tokens.{id_token,access_token,refresh_token,account_id}`),
  `~/.pi/agent/auth.json` (`openai-codex: {type:"oauth", access, refresh, expires em ms, accountId}`)
  e a tabela `auth_credentials` do `~/.omp/agent/agent.db` (`provider`, `credential_type='oauth'`,
  `data` = a credencial do Pi sem o `type`, `identity_key` = accountId; conferido com
  `omp token openai-codex` devolvendo JWT). O app roda o fluxo de dispositivo sozinho (stdlib), guarda
  em `~/.hangar/auth/openai-codex.json` (0600) e escreve nos três — **só onde não há login**; login
  existente é da pessoa. Três medições que decidem o desenho:
  - **Rotação do refresh não invalida a cópia.** Renovando pelo refresh do Codex por fora, a resposta
    trouxe refresh NOVO, o antigo continuou renovando e o Codex seguiu autenticando com o store
    intocado (o 400 seguinte foi de modelo, não de auth). Por isso cada CLI renova sozinho, sem
    renovador central; o custo de um dia isso mudar é a linha "Login" do painel ficar vermelha.
  - **A Cloudflare do `auth.openai.com` devolve 530 (`cf_route_error`) pro User-Agent padrão do
    urllib.** Qualquer outro UA passa; `_http` manda `hangar/1.0`.
  - **`earliest_refresh_at` na resposta do token**: o servidor diz quando o próximo refresh é aceito
    (~9 dias, com o access valendo 10). Não é erro, é o ritmo dele.
  O `codex login` (0.153.1) não tem `--device-auth` visível no `--help`; o app não depende dele.

## Proibição de instalar o Windows como administrador

Em 10/09/2026, o instalador passou a recusar execução elevada: tarefas criadas com dono
Administradores impediam que a atualização comum usasse `Register-ScheduledTask -Force`.
Em 12/09/2026, a proibição foi substituída por níveis coerentes: uma instalação elevada
registra tarefas interativas `Highest` e atalhos elevados; a atualização herda o backend.
O problema medido era misturar permissões, não uma incapacidade do psmux elevado de colar.
Regra atual em [instalacao.md](instalacao.md#instalação-windows-mantém-o-nível-de-permissão).

## Lançador Windows sem acompanhamento e vigia baseada só em porta

O `.vbs` usava `Run(..., 0, False)`: a tarefa terminava enquanto o Python continuava vivo.
A vigia consultava porta TCP e idade dos processos; após dez minutos sem porta, iniciava
outra instância sem encerrar a anterior. Em 12/09/2026, esse fluxo foi substituído por tarefa
que aguarda o filho, recuperação nativa e vigia HTTP com parada seletiva. Evidência e limites
da validação em [instalacao.md](instalacao.md#a-tarefa-windows-acompanha-o-processo-até-ele-terminar).

## O jev-gateway como caminho da conversa

Entre 19 e 20/09/2026 o Hangar sabia pôr uma sessão atrás do `jev-gateway` (projeto externo,
checkout em `~/.local/share/jev-gateway`): `ANTHROPIC_BASE_URL` no Claude, `model_provider` por
`-c` no Codex sem terminal, campo `jev_gateway` na criação, `--jev-gateway` no `hangar-send`,
caixa na folha e padrão por servidor. O gateway perguntava ao Jev qual tool o turno pedia e, com
confiança, forçava `tool_choice` (Codex) ou sugeria (`hint`, no Claude, porque raciocínio ligado e
cache impedem forçar).

Saiu em 20/09/2026 por decisão do Jefferson, e o motivo é de desenho, não de medição: para
escolher UMA tool, a conversa inteira do turno passava por um terceiro. Preço desproporcional ao
que se ganhava — e o que se ganhava, medido, era pouco ou negativo:

- **Codex** (0.154.0, `gpt-5.6-sol` high, criação de tela, uma execução por lado): saída +0,9%,
  entrada −11,8%, tempo +8%. As tools vêm embrulhadas numa `exec` de JavaScript, então o gateway
  via 3 tools de topo e a escolha real acontecia dentro do script, fora do alcance dele.
- **Claude** (Claude Code 2.1, `claude-fable-5-1` high, depuração com 7 bugs plantados e 16 testes,
  uma execução por lado, tokens somados do `usage` por `message.id`): 17 pedidos ao modelo contra
  9, entrada 1.082.854 contra 630.157 (**+72%**), saída 3.671 contra 3.158 (+16%), 2min53 contra
  55s. Os dois lados fecharam 16/16. Uma execução por lado não separa isso de variação entre
  rodadas — era por isso que a opção nascia desligada.

Duas coisas que a remoção também leva embora: a sonda de porta antes de subir a sessão (provedor
apontando para porta fechada é sessão que nasce e nunca fala com o modelo) e a degradação calada
no relançamento, que trocava o regime de custo sem nada na tela dizer em qual regime a sessão
estava. Ficou por conferir, e agora não será: `thread/resume` de uma thread criada com o gateway
numa subida sem ele.

O `jev` da sessão CONTINUA — é outra coisa: só põe a chave da TypeSafe no ambiente, para o
`hangar-preview objetivo` navegar sozinho. Regra atual em
[harnesses.md](harnesses.md#regras-vigentes).

## Function hooks fora do Hangar

Em 14/09/2026 os function hooks do Claude Code foram medidos e deixados de fora: a API é de
acesso antecipado e muda sem aviso, e o interruptor `claude_function_hooks` só punha a variável
no ambiente para o plugin de quem usa o Hangar. Em 18/09/2026 o Hangar passou a carregar um
plugin próprio (`plugins/hangar`) atrás do mesmo interruptor, como caminho opcional por cima do
tmux: o risco da API virou fallback por ausência, não motivo para não usar. Regra atual e
medições em [harnesses.md](harnesses.md#function-hooks-o-plugin-pluginshangar-é-um-plus-por-cima-do-tmux-18092026).
