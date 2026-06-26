---
branch: main
saved_at: 2026-06-25T22:35:00-03:00
saved_commit: 0eb768db3ba1ce3cf424b4555d2ebd8a39f97552
status: in_progress
---

## TL;DR
claude-pocket (dirigir Claude Code do celular). Sessão MARATONA de dogfooding ao vivo via app.
Entregue e funcionando: redesign composer/teclado/fila/badge + **Phase 4** (métricas: 5h/7d na
NavBar, custo no topo, UsageSheet) + **upload de imagem** (📎 + paste, lazy, salva em cwd) +
**preview de imagem no chat** (serve endpoint + ImageBubble) + cadeia de fixes de confiabilidade
(401 self-heal + token estável; 500-fix do newline; feedback de erro visível; guard #undefined) +
**fix do teclado iOS** (layout viewport-sized — NavBar fixa topo + composer fixo bottom + lista
único scroller — CONFIRMADO no aparelho). **PRÓXIMO: contenteditable** pra matar a barra de
acessório do iOS (`^ v ✓` acima do teclado). 13 commits LOCAIS (vou pushar agora). Stack roda.

## Task atual
PRÓXIMO passo decidido: **refatorar a textarea do Composer pra `<div contenteditable>`** — campos
nativos (input/textarea) disparam a barra de acessório do iOS; contenteditable não. Mata a barra
inteira (`^ v` + Done). Reusar Enter/paste/auto-grow; placeholder vira CSS; gerenciar textContent
na mão (perde bind:value). Testar no aparelho (user manda print). Depois: fila durável no servidor.

## Concluído nesta sessão
- Redesign + Phase 4 (ver handoff anterior / git 90bfc16..b4ce6de).
- Dogfooding: 401 self-heal (33f002b) + token estável backend/.env; composer sempre-visível
  (cdf0443); parser AskUserQuestion scoped (c130947, MAS não surge no app — debug pendente);
  guard #/chat/undefined (26e2194).
- **Upload de imagem**: save_upload + POST /upload raw-body (916d6db, abf48c1); uploadImage +
  gitignore (d3b0948, 4485d1f); Composer 📎+preview+upload-on-send (de8bb43); paste (6690acc).
- **500-fix** (ff74def): input vira 400 (não 500) + msg de imagem UMA LINHA (`<legenda> — 📎
  imagem: <path>`; newline → send_prompt rejeita control char → era 500 e comia a msg).
- **Feedback de erro** (706eb20): erro de envio visível, preserva input na falha (onSend awaitable).
- **Fix teclado** (48d52e3): Chat vira flex viewport-sized (screenEl.height=vv.height +
  translateY(offsetTop)); NavBar/composer flex-shrink:0; MessageList único scroller. SEM
  fixed-dock+translateY. CONFIRMADO: NavBar não some mais.
- **Preview de imagem** (7f00737, 63f5693, 0eb768d): resolve_upload + GET /uploads/{filename}
  (cookie auth — require_auth lê cp_token) + ImageBubble + parseImageMessage. Prints viram imagem.

## Decisões
- **Imagem = lazy upload-on-send** (sobe no envio, sem órfão se cancelar). Formato UMA LINHA
  obrigatório (`\n` → 400). Assistente Lê o path; app renderiza via GET /uploads (cookie, same-origin).
- **Teclado = viewport-sized flex** (não fixed+translateY). screenEl rastreia vv.height+offsetTop.
- **Barra acessório iOS** = só some com `contenteditable` (próximo passo) OU no PWA standalone.
- **Token estável** em backend/.env (gitignored); test_config hermético (public_url="").

## Limitações conhecidas
- **AskUserQuestion NÃO surge no app** (classify scoped + unit-tested, mas os botões não aparecem —
  provável sessão-observada-errada ou pane vivo ≠ fixture). Com user: usar TEXTO NUMERADO, não AskUserQuestion.
- **Sem fila durável**: sendInput é send-keys imediato; "fila" = nativa do Claude (só enquanto
  gera; num picker o texto some) + bubbles pendentes locais (somem no reopen). User quer fila no
  servidor (entrega quando o pane aceita, sobrevive tudo). Design pendente.
- **Multi-linha de texto** não suportada (send_prompt rejeita `\n`; Shift+Enter → 400). Backlog.
- **Uploads acumulam** em .claude-pocket-uploads/ — falta retention sweep (backlog).
- AskUserQuestion options incluem meta ("Type something"/"Chat about this") — não filtradas.
- Paste de imagem no iOS é instável (preview aparece, envio pode falhar) — no iPhone usar 📎.

## Erros / armadilhas
- send_prompt rejeita control chars (`\n`) → input_prompt agora 400 (era 500 silencioso que comia msg).
- Restart backend: `pkill -f app.main` numa linha com o relaunch teve RACE (matou o novo). Usar
  `kill <pid>` do antigo + guard `ss | grep :8765 || relaunch`.
- iOS PWA: token em localStorage (isAuthenticated só checa existência) → token velho trava em 401 →
  self-heal (api.ts: 401 c/ token → clearCredentials + reload). Cookie do Safari não passa pro PWA.
- Testar AskUserQuestion/picker: loop capture-pane em background ENQUANTO dispara (ela bloqueia).

## Arquivos criticos
- Backend (N/R): app/uploads.py (N: save_upload+resolve_upload), app/api.py (R: upload/serve/input400),
  app/state.py (R: _menu_block), backend/.env (N gitignored), tests/{test_uploads,test_state_classifier,test_config}.py.
- Frontend (N): components/{RateChips,UsageSheet,ConfirmSheet,Spinner,ImageBubble}.svelte; lib/format.ts(parseImageMessage).
- Frontend (R): components/{Composer(📎+paste+feedback+metrics-top),MessageList(imagem+sticky),NavBar(chips)}.svelte,
  screens/Chat.svelte (flex viewport layout, sem dockEl), App.svelte (guard undefined), lib/api.ts (uploadImage+401).
- Docs: future-features.md, plans/specs (phase4, image-upload, composer-redesign).

## Próximo passo
```
# 1. (FEITO neste save) commit handoff + docs + PUSH dos 13 commits:
git -C . add -A && git -C . commit -m "docs: handoff + image-upload plan/spec" && git -C . push origin main
# 2. Stack: backend :8765 (lê backend/.env, token B_cCngF3YyM31J3CAOMMK9-e), vite :5173 --host, tailscale serve.
#    Restart backend (sem race): OLDPID=$(pgrep -f app.main|head -1); kill $OLDPID; sleep 1; (cd backend && uv run python -m app.main &)
# 3. PRÓXIMA TASK: refatorar Composer textarea -> <div contenteditable> (mata a barra de acessório iOS).
#    Reusar handleKeydown/onPaste/autoGrow; placeholder via CSS :empty::before; gerenciar textContent.
# 4. Depois: fila durável no servidor; filtrar meta-options; debug AskUserQuestion-no-app; uploads retention.
# resume: /handoff resume  (após git pull)
```
