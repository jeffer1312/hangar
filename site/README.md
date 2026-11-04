# site

Landing page de `hangar.dev.br`: PT em `/`, EN em `/en/`, gerada em `site/public/` (fora do git).

## Gerar e ver

```bash
python3 site/build.py
python3 -m http.server -d site/public 8099
```

## Testar

Só quando pedido.

```bash
python3 -m unittest discover site/tests
node --test site/tests/site.test.mjs
```

## Publicar

Uma vez só, antes do primeiro upload: crie o projeto pela API (o `wrangler pages project
create` atual cai no Workers e pede um subdomínio workers.dev) e faça o login do wrangler (o da
`cf` não serve para upload):

```bash
CLOUDFLARE_ACCOUNT_ID=<id da conta> cf pages create --body '{"name":"hangar-site","production_branch":"main"}'
npx --yes wrangler login
```

Depois:

```bash
site/deploy.sh          # prévia (preview.<subdomínio>.pages.dev)
site/deploy.sh main     # produção
```

Não troque o `deploy.sh` por wrangler na raiz: ela é workspace npm e o wrangler recusa.

## Cache

Tudo em `/assets/media/` (vídeos, capas, imagens, `og.png`) sai `immutable` por um ano
(`site/static/_headers`). Arquivo trocado precisa de nome novo.

## Regravar vídeos

Só Linux/Hyprland. Abre janelas na tela de quem grava.

```bash
python3 site/tools/record/live.py &
site/tools/record/record.sh <cena> <segundos> <ctrl-down> <lang>
site/tools/record/enc.sh <cena> <lang>
```

| cena | segundos | ctrl-down |
|---|---|---|
| `agents` | 15 | 1 |
| `ask` | 16 | 1 |
| `pair` | 15 | 1 |
| `orq` | 13 | 4 (seleciona `orq-checkout`, a 4ª linha da lateral) |

- `<lang>` é `en` ou `pt`. A saída fica em `site/tools/record/out/`; copie `<cena>-<lang>.mp4`
  e `.jpg` com nome NOVO (`agents-v2.mp4`/`agents-v2.jpg`) para `site/media/` (EN) ou
  `site/media/pt/` (PT) e troque o `video`/`poster` da cena em `CLIPS` (`site/src/strings.py`,
  posição PT ou EN): o cache `immutable` serviria o vídeo antigo com o nome antigo.
- A janela tem 1600x960 na posição 160,60: o monitor precisa de pelo menos 1760x1020 lógicos
  (tamanho dividido pela escala). `launch.sh` recusa se não couber.
- Variáveis: `HANGAR_RECORD_MONITOR=<nome>` escolhe o monitor (padrão: o focado);
  `HANGAR_RECORD_SIZE`, `HANGAR_RECORD_POS`, `HANGAR_NATIVE_BIN` (padrão: `hangar-native` no
  `PATH`) e `HANGAR_RECORD_OUT` mudam tamanho, posição, binário e pasta de saída.

## Relatórios de falha (`worker/`)

Worker de `hangar.dev.br/api/relatorio`: recebe o relatório do assistente de instalação, guarda no KV por 90 dias e manda
e-mail. Projeto npm próprio (fora dos workspaces da raiz).

Dois tipos de relatório, pelo campo `kind` do corpo:

- **Instalação** (sem `kind`): carimbo `hangar-native/<versão>`, chave `<data>/<uuid>`, limite de 3 por minuto por IP.
- **Diagnóstico** (`kind: "diagnostico"`, o "Reportar problema" das Configurações): o servidor do Hangar envia com o
  carimbo `hangar-server/<versão>`. Chave `diag/<código>` (8 letras de `a-z2-7`), limite próprio de 1 por minuto por IP
  (`DIAG_LIMITER`). Sem gravar no KV (falha ou teto do dia) a resposta é 503, nunca um código que não abre. A resposta
  `201 {id, code, upload}` traz uma senha de uso único (10 min) para o envio do diário inteiro:
  `PUT /api/relatorio/diario` com `x-hangar-stamp`, `x-hangar-code: <código>`, `x-hangar-upload: <senha>` e
  `content-length` até 10 MiB. O corpo (gzip) vai ao KV sem ser aberto, em `diag/<código>/diario.jsonl.gz`.
  Respostas: 201, 403 (carimbo, código ou senha errada/vencida/já usada), 411 (sem `content-length`), 413, 429, 503.
- **Diagnóstico de várias máquinas** ("todas as minhas máquinas"): o primeiro relatório recebe também `group_token`, a
  assinatura do código dele (HMAC com o segredo `GROUP_SECRET`, vale 30 min). As outras máquinas mandam
  `x-hangar-group: <código do primeiro>` e `x-hangar-group-token`; o worker confere sem ler o KV, não cobra o limite
  por IP, limita 7 por minuto por grupo (`GROUP_LIMITER`) e 10 por minuto no total delas (`GROUPS_LIMITER`), e grava
  `diag/<código>` com `group` e a marca `diag/<primeiro>/m/<código>`. Respostas próprias: 400 `grupo` (assinatura
  errada ou vencida) e 429 `grupo-cheio`. As máquinas de um envio: `npx wrangler kv key list --binding REPORTS
  --prefix "diag/<primeiro>/m/" --remote`.

O e-mail do diagnóstico chega com o assunto `[Hangar] diagnóstico <código> · <sistema>`. Para abrir o relatório e baixar o
diário:

```bash
cd site/worker
npx wrangler kv key get --binding REPORTS "diag/<código>" --remote
npx wrangler kv key get --binding REPORTS "diag/<código>/diario.jsonl.gz" --remote > diario.jsonl.gz
```

```bash
cd site/worker && npm ci
npm test            # só quando pedido
npm run check
```

Pré-requisitos para publicar (nada disso está no repositório):

1. Um namespace KV criado na conta; o `id` dele substitui o texto provisório de `kv_namespaces` em `worker/wrangler.jsonc`.
2. Email Sending/Routing ativo para o domínio `hangar.dev.br`, com o endereço de destino já verificado.
3. O segredo `REPORT_TO` (endereço de destino, `wrangler secret put REPORT_TO`) e as rotas `hangar.dev.br/api/relatorio` e
   `hangar.dev.br/api/relatorio/diario` na zona.
4. O segredo `GROUP_SECRET` (valor aleatório longo, `wrangler secret put GROUP_SECRET`). Sem ele o envio de várias
   máquinas não agrupa: só o primeiro relatório sai, e as outras máquinas aparecem como sem suporte.
5. Os limitadores do `wrangler.jsonc` (`LIMITER` 1001, `GLOBAL_LIMITER` 1002, `DIAG_LIMITER` 1003, `DIARY_LIMITER` 1004,
   `GROUP_LIMITER` 1005, `GROUPS_LIMITER` 1006): o deploy os cria.

```bash
cd site/worker && CLOUDFLARE_ACCOUNT_ID=<id da conta> npx wrangler deploy
```
