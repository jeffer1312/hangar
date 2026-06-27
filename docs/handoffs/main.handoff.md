---
branch: main
saved_at: 2026-06-26T16:30:00-03:00
saved_commit: b2b8c808d1cac545d2234ece0157bd0d43142557
plan:
status: in_progress
---

## TL;DR
claude-pocket — sessão GIGANTE de dogfooding mobile. TUDO pushado (HEAD b2b8c80, `main` em dia,
5 commits 67283dc..b2b8c80). Backend rodando :8765 (pid 1653686). Entregue: streaming preview ao
vivo, SSE heartbeat/staleness, fix duplicação, teclado/glass/preto iOS (research-backed), /proc
session resolution, sidebar (rename/badge/2-tap), markdown tabelas, multi-linha, preview universal de
arquivos, USAGE.md+Tailscale. **FALTA: validar os fixes iOS no iPhone de verdade** (não confirmado).

## Task atual
Acabou de sair o preview universal de arquivos (vídeo thumb 96px→full). Tudo commitado/pushado.
Próximo: usuário testar no iPhone (recarregar PWA) os fixes de teclado/preto/streaming/preview.

## Concluído nesta sessão
(git 67283dc..b2b8c80 — 5 commits: e0efeec backend, 90a9d5a frontend, 818f163 USAGE, 922cb78 tailscale, b2b8c80 file-preview)
- **Streaming preview**: app/preview.py (extrai prosa do pane via capture, pula tool-lines), PreviewBroker
  COMPARTILHADO por sessão, SSE lane coalescida + suppress do já-commitado. Front: box contido (cauda
  desliza dentro), clear-on-commit + idle (lê previewText SEMPRE — Svelte5 só rastreia dep lida).
- **SSE staleness**: heartbeat 'ping' visível (10s) + watchdog (25s) + reconnect no visibilitychange.
- **Estado**: debounce working→idle (IDLE_DEBOUNCE=4) — capture mid-redraw não pisca mais.
- **iOS preto/glass/teclado** (2 workflows de pesquisa): glass isolado em .composer-card::before;
  NavBar camada própria; dock fixado via `top: vv.offsetTop` (NÃO transform → sem preto, sem prender
  sheets fixed); rAF-coalesce scroll; focusout reset (resíduo iOS 26).
- **/proc session resolution**: registry resolve o jsonl que o processo claude tem ABERTO (não
  newest-by-mtime) → sessão nova vem limpa.
- **Sidebar**: renomear por toque-longo (+ endpoint /rename), dot "pensando" pulsa, fix 2-tap (hover:hover).
- **Markdown**: tabelas/listas/headings/links (+ fix de comentário `*/` que quebrava o módulo).
- **Multi-linha**: send_prompt via bracketed paste. **Imagem do terminal**: /transcript-image (base64).
- **Preview universal**: /file?path= (só serve path citado no transcript; /etc/passwd→403; Range) +
  FileAttachment (img/vídeo thumb→full, html/pdf iframe modal). docs/USAGE.md + Tailscale (Context7).

## Decisões
- Streaming token-real (stream-json/SDK) = HEADLESS-ONLY → mataria o REPL → FORA. Fonte = capture-pane.
- iOS: `top:offsetTop` (layout, não compositing) vence transform — transform = camada tiled-backing
  (preto WebKit 220892/226532) E vira containing-block que prende os 6 sheets position:fixed.
- Preview suppress: front é a verdade (sabe o que mostrou), backend é bônus. clear-on-commit > match texto.
- File preview: trava de segurança = só path no transcript. Código/texto NÃO auto-anexa (ruído na prosa).
- Usuário DESATIVOU o fish auto-tmux (comentou em ~/dotfiles/.../config.fish) — re-anexava em sessão velha.

## Limitações conhecidas
- **Fixes iOS NÃO confirmados no device** (teclado top:offsetTop, preto sumiu, vídeo thumb→full).
- Preview = CAUDA do fullscreen (resposta longa só mostra o fim). Lever CLAUDE_CODE_DISABLE_ALTERNATE_SCREEN
  não aplicado (muda o terminal do usuário).
- File preview: html com assets relativos não renderiza 100% (serve 1 arquivo). Código/texto sem preview.
- Markdown: listas aninhadas viram flat. Dotfiles (fish) em repo separado (~/dotfiles).

## Arquivos criticos
- Backend (N): app/preview.py. (R): app/{sse.py (heartbeat+preview lane+suppress), state.py (debounce),
  registry.py (/proc), transcript.py (path_in_transcript+image), api.py (/file,/transcript-image,/rename),
  terminal_input.py+tmux.py (multiline+rename+pane_pid+paste), pqueue.py (clear no /clear)}.
- Frontend (N): components/FileAttachment.svelte. (R): screens/Chat.svelte (preview-clear+teclado
  top:offsetTop+SSE watchdog), MessageList.svelte (preview box+anexos), Composer.svelte (glass ::before),
  NavBar.svelte, Sidebar.svelte (rename/badge/2-tap), AssistantBubble.svelte, lib/{markdown.ts,format.ts,api.ts}.
- Docs (N): docs/USAGE.md.

## Próximo passo
```
# 1. Usuário testa no iPhone (recarregar PWA, idealmente hard-reload p/ furar cache do SW):
#    teclado abre -> dock COLADO acima dele, NavBar no topo, sem vão; streaming sem pulo;
#    sheets (slash/modelo/switcher) cobrem tela com teclado aberto; vídeo thumb -> tap -> full;
#    rolar 30s durante streaming -> SEM retângulo preto no topo.
# 2. Restart backend (cwd=backend, sem --reload):
#    PID=$(ss -tlnp|grep :8765|grep -oP 'pid=\K[0-9]+'); kill -9 $PID
#    ( cd backend && CLAUDE_CONFIG_DIR=~/.claude-work setsid .venv/bin/python3 -m app.main >server.log 2>&1 & )
# 3. Backlog: preview de código/texto por marcador (📎 arquivo:); lever alternate-screen p/ stream completo;
#    unificar sidebar desktop com visão agregada multi-server; SW network-first em dev (cache trap).
# Stack: backend :8765 (uv/.venv), vite :5173 --host, tailscale serve -> vite. Token em backend/.env.
```
