# Design — Upload de imagem do celular pra sessão (v1)

**Data:** 2026-06-25
**Escopo:** v1 = **só enviar** (o assistente vê via Read). Sem render persistente, sem múltiplas
imagens, sem áudio (backlog). Desbloqueia o uso remoto: o usuário (longe do PC) manda um print
pelo app pra mim ver.

## Problema
O usuário dirige o Claude Code pelo celular (app claude-pocket) e, remoto, não tem como me mandar
um screenshot pra eu ver um bug (ex: o bug do teclado/top-bar no iOS). O terminal aceita paste de
imagem, mas o app não tem upload. Mecânica nativa de paste de imagem do Claude Code **não é
dirigível via tmux** (send-keys é texto). Então: salvar a imagem num arquivo + me passar o path,
e eu uso o **Read** (que renderiza imagens) pra ver.

## Solução (Opção A: path + Read)
1. App tira/escolhe a foto → `POST /api/sessions/{name}/upload` (multipart).
2. Backend salva num dir permitido dentro do cwd da sessão → devolve `{ path }`.
3. App manda um input referenciando o path (+ legenda) pelo endpoint `/input` que já existe.
4. Eu recebo a mensagem com `📎 imagem: <path>` → uso o Read no path → vejo o screenshot.

### Backend
- **Endpoint** `POST /api/sessions/{name}/upload` (FastAPI `UploadFile`, campo `file`),
  `dependencies=[Depends(require_auth)]` (mesmo padrão dos outros).
- **Validação:**
  - content-type em `{image/png, image/jpeg, image/webp, image/gif}` (senão 415).
  - tamanho ≤ 10 MiB (senão 413). Ler em streaming/checar `len`.
- **Resolução do cwd:** a sessão tem cwd via `tmux list-sessions -F '#{session_name}\t#{pane_current_path}'`
  (já usado em `tmux.list_sessions()`/registry). Pegar o cwd da sessão `name`. Se a sessão não
  existe → 404; se cwd vazio/irresolvível → 409.
- **Destino:** `<cwd>/.claude-pocket-uploads/`. Criar se faltar (`mkdir -p`).
- **Nome do arquivo:** `<unix_ts>-<rand6>.<ext>` onde ext deriva do content-type (png/jpg/webp/gif).
  NUNCA usar o filename do cliente (path traversal). 
- **Containment (segurança):** `realpath(destino)` tem que começar por `realpath(<cwd>/.claude-pocket-uploads/)`
  — rejeita se escapar (espelha o padrão de `fs.py` do scanner). 
- **Retorno:** `{ "path": "<abs path>" }` (200).
- **Sem injeção:** o endpoint só salva. Quem manda a mensagem pro pane é o app (via `/input`),
  pra o usuário controlar legenda e momento.
- **Arquivo novo** `app/uploads.py`: `save_upload(cwd, content, content_type) -> str` puro-ish
  (IO de arquivo), testável. `app/api.py` só faz o glue (auth, validação HTTP, resolve cwd, chama).

### Frontend
- **`lib/api.ts`**: `uploadImage(name, file: File): Promise<{ path: string }>` — `FormData` com
  `file`, POST pro endpoint, headers de auth (sem `Content-Type` manual; o browser põe o boundary).
  401 cai no self-heal existente.
- **`Composer.svelte`:**
  - Botão 📎 (`attach-btn`) no `control-left` (perto do `/`). `<input type="file" accept="image/*">`
    escondido; o botão dá `.click()`. No iOS isso oferece Câmera/Galeria.
  - `onchange` → guarda o `File` em `attachment = $state<File|null>`, gera `objectUrl` p/ preview.
  - **Chip de preview** acima da textarea quando há anexo: thumbnail + nome + botão "x" (remove).
  - A textarea segue como **legenda**.
  - **`submit()`** com anexo: `canSend` passa a aceitar (texto não vazio **OU** anexo). Fluxo:
    1. `uploadImage(sessionName, attachment)` → `{ path }`.
    2. `onSend("<legenda>\n📎 imagem: " + path)` (legenda pode ser vazia).
    3. limpa `attachment`/`objectUrl`/textarea.
    4. mostra **bubble otimista** com a imagem (object URL) — efêmero (some no reload, ok p/ v1).
  - **Erro de upload:** mostra erro inline, mantém o anexo (não perde a foto).
  - Revogar o `objectUrl` no remove/após enviar (evita leak).
- **Bubble otimista:** reusar `pending` do Chat? Não — pending é texto. Adicionar um caminho
  simples: o Composer emite via um callback `onAttachmentSent(objectUrl, caption)` que o Chat
  empurra numa lista local de "imagens enviadas" renderizada na MessageList. (Decisão de detalhe
  fica pro plano; o mínimo aceitável é só o `sendInput` + o texto aparecer no transcript — o bubble
  otimista é nice-to-have dentro do v1.)

### `.gitignore`
- Adicionar `.claude-pocket-uploads/` (a subpasta de uploads não vai pro repo).

## Testes / sucesso
- **Backend (pytest):** `save_upload` cria o dir e grava; rejeita ext/content-type inválido;
  containment rejeita escape; nome não usa o filename do cliente. Endpoint: 200 c/ path válido,
  415 content-type ruim, 413 grande demais, 404 sessão inexistente. (mockar cwd/tmux como nos
  outros testes.)
- **Frontend:** `npm run check` + `build` limpos. Manual no aparelho: 📎 → escolher foto →
  preview → enviar → o assistente responde tendo visto a imagem (Read no path).
- **Critério real:** o usuário manda o print do bug do teclado e eu descrevo o que vejo.

## Fora de escopo (backlog)
- Render persistente das imagens no app (GET p/ servir + bubble de imagem do transcript no reload).
- Múltiplas imagens por mensagem; arquivos genéricos; áudio (voz).
- Debug separado: **AskUserQuestion não aparece no app** (o classify foi escopado e unit-testado,
  mas na prática os botões não surgem — provável sessão-observada-errada ou pane vivo ≠ fixture).

## Segurança
- Auth em todo endpoint. Containment realpath dentro de `<cwd>/.claude-pocket-uploads/`.
- Nome de arquivo gerado pelo servidor (sem traversal). Limite de tamanho + allowlist de
  content-type. CDP/Tailscale já dão o canal seguro (HTTPS no tailnet).
