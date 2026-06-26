---
branch: main
saved_at: 2026-06-25T21:05:00-03:00
saved_commit: 26e21945d9882b1d27b7adc5be08eb165d36e359
status: in_progress
---

## TL;DR
claude-pocket (dirigir Claude Code do celular). ESTA sessão (longa, dogfooding ao vivo via app):
fechou o redesign composer/teclado/fila/badge + **Phase 4** (métricas restauradas: 5h/7d na
NavBar, custo no topo do composer, UsageSheet) + uma CADEIA de bugs de dogfooding (401 self-heal
+ token estável; composer sempre-visível anti-trava; **AskUserQuestion agora vira OptionButtons
no app** — CONFIRMADO funcionando; guard #/chat/undefined). **16 commits LOCAIS — PUSH PENDENTE.**
Stack roda (backend :8765 lê backend/.env, vite :5173, tailscale serve). Falta: filtrar opções
meta, bug teclado/top-bar iOS (precisa screenshot — user remoto, sem upload ainda).

## Task atual
Phase 4 + fixes de dogfooding essencialmente prontos e verificados no app (chips, UsageSheet,
stop-confirm, spinner slim, AskUserQuestion→botões, composer anti-trava). Próximo: PUSH dos 16
commits; polish (filtrar meta-options "Type something"/"Chat about this"); bug teclado iOS
(bloqueado em screenshot); backlog (painel agents/workflows + TodoWrite; upload de imagem).

## Concluído nesta sessão
- Redesign (90bfc16..e1d83d7): teclado iOS (lock doc + visualViewport offsetTop/scroll/focus),
  sticky-scroll, fila com pending bubbles + dedup, composer input-puro, ActivityBadge.
- **Phase 4** (f536266..b4ce6de + 43d1764): RateChips (5h/7d na NavBar), UsageSheet (detalhe +
  raw fallback), ConfirmSheet genérico, Spinner slim (renomeia ActivityBadge, sem cost/stop),
  wiring, cost→topo do composer. Disposição = Opção 1 (espalhado; ctx no ring, modelo no pill).
- Dogfooding fixes: 401 self-heal (33f002b) + token estável em backend/.env; composer
  SEMPRE visível anti-trava (cdf0443); **parser AskUserQuestion** (c130947 — _menu_block escopa
  ao box do picker, sem poluição de scrollback; 103 testes verdes); guard #/chat/undefined (26e2194).
- Backlog (docs/future-features.md): item 3 (surface UI interativa), item 4 (fixes), painel
  TodoWrite, prioridade upload. Memória: claude-pocket-app-interaction.

## Decisões
- **Token estável** em backend/.env (CP_AUTH_TOKEN, gitignored) → restart não desloga. Teste
  test_config tornado hermético (public_url="") porque .env vazava no Settings().
- **AskUserQuestion usa o caminho EXISTENTE** awaiting_input→OptionButtons. O fix foi ESCOPAR a
  extração de opções ao bloco do menu (âncora: cursor ❯ N. + rodapé "Enter to select · ↑/↓ to
  navigate · Esc to cancel"; limites: boundary ●/⎿/spinner). Funciona p/ AskUserQuestion (tem
  rodapé) E menu nativo de permissão (sem rodapé, fecha no boundary). Renderiza NO pane (capture-pane vê).
- **Composer sempre visível** (nunca esconde em awaiting_input) — anti-trava; OptionButtons
  aparecem na lista como adição. Risco de free-text-no-menu aceito vs lock-out.

## Limitações conhecidas
- Bug teclado/top-bar iOS NÃO corrigido — precisa screenshot; user remoto sem upload de imagem
  (chicken/egg → upload virou prioridade no backlog).
- Opções da AskUserQuestion incluem as meta "Type something."/"Chat about this" (não filtradas).
- cost-no-topo "podia ficar melhor" (user refina quando tiver upload p/ mandar print).
- `undefined` 404 era ruído de SSE fantasma no log do backend; user NUNCA viu na tela; guard
  adicionado defensivamente.

## Erros / armadilhas
- backend/.env (CP_PUBLIC_URL) vaza no Settings() de teste → quebrou pairing_url; fix: public_url="".
- iOS PWA: token em localStorage; isAuthenticated() só checa existência, não validade → token
  velho trava o app em 401. Self-heal (api.ts: 401 com token → clearCredentials + reload → Login).
- iOS PWA: cookie/localStorage do Safari NÃO transfere pro PWA instalado → re-pair tem que ser
  no mesmo contexto (scanner QR do app).
- Testar AskUserQuestion: loop de capture-pane em background ENQUANTO dispara a AskUserQuestion
  (ela bloqueia), depois lê a captura (fixture pane_askuserquestion.txt).

## Arquivos criticos
- Backend (R/N): app/state.py (R — _menu_block + _question escopado), backend/.env (N gitignored,
  token estável), tests/fixtures/pane_askuserquestion.txt (N), tests/{test_state_classifier,test_config}.py (R), app/config.py (env_file=".env").
- Frontend (N): components/{RateChips,UsageSheet,ConfirmSheet,Spinner}.svelte.
- Frontend (R): components/{NavBar,Composer,MessageList}.svelte, screens/Chat.svelte, App.svelte
  (router guard), lib/api.ts (401 self-heal), lib/auth.ts.
- Docs: future-features.md (R), plans/2026-06-25-phase4-metrics-restore.md (N),
  specs/2026-06-25-composer-keyboard-queue-badge-design.md (N).

## Próximo passo
```
# 1. PUSH (16 commits locais, nada no remote desta sessão):
git -C . push origin main      # range 5f7f3c6..HEAD
# 2. Stack já roda: backend :8765 (lê backend/.env, token B_cCngF3YyM31J3CAOMMK9-e), vite :5173 --host,
#    tailscale serve. Se backend caiu: cd backend && uv run python -m app.main  (lê .env, mesmo token).
# 3. Pendentes:
#    - filtrar meta-options no state.py (dropar "Type something."/"Chat about this" do options)
#    - bug teclado/top-bar iOS (ESPERAR upload de imagem p/ screenshot)
#    - backlog: painel agents/workflows + TodoWrite (transcript); upload de imagem (prioridade)
# resume: /handoff resume  (após git pull)
```
