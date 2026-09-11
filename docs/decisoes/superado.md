# Superado — mecanismos que não existem mais

Estes trechos descrevem como algo **funcionava antes**. Ficam registrados porque a
medição continua valendo como história, mas não descrevem o código de hoje: cada um
aponta a decisão que o substituiu. Não leia daqui para decidir implementação.

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
