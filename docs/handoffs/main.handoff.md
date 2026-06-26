---
branch: main
saved_at: 2026-06-26T12:42:20-03:00
saved_commit: 7bca67493bba233f2d658c1d37ba99b92b099a1f
status: in_progress
---


## TL;DR
claude-pocket (dirigir Claude Code do celular + AGORA desktop). Sessão enorme de dogfooding. Tudo
pushado (HEAD 7bca674), buildando (svelte-check 0 erros), backend reiniciado (pid 1458939, :8765).
Entregue: polish mobile completo, **multi-PC cross-origin funcionando**, **painel de atividade +
drill-in workflow→agente**, **shell desktop** (sidebar + chat largo, mobile intocado), **fix truecolor
do tmux**. O pull deles (multi-server aggregated) foi integrado via stash/pop limpo.

## Task atual
Acabou de sair o shell desktop (≥820px). Multi-PC OK. tmux truecolor aplicado + backend restartado.
Próximo: validar o desktop no PC + (opcional) unificar a sidebar desktop com a visão agregada deles.

## Concluído nesta sessão
(git fcbafe1..7bca674 — muitos commits; ver `git log`)
- Polish mobile: glass iOS, scroll dinâmico anti-glitch, multi-imagem, merge stop/send, lightbox(portal),
  fila durável (atenua→acende), K 1→4 (preto no scroll iOS).
- Filtros de meta no transcript: skill-inject + `<task-notification>` (vazavam como bubble).
- Painel de atividade (lib/activity.ts: fold de TaskCreate/TaskUpdate + agents) + ActivitySheet; drill-in
  de workflow (backend workflows.py lê os arquivos do run no disco) → fases+agentes → agente
  (prompt + resultado completo + tools). 3 níveis.
- Multi-PC: auth.ts multi-server + switcher; CORS no backend; vite `cors:false` + plugin apiCorsPreflight
  (preflight OPTIONS); auth.py lê `?token` (SSE cross-origin); destravou o "não vejo minha msg".
- Desktop (≥820px): App.svelte → DesktopShell (Sidebar + Chat largo); media queries 600→920 ADITIVAS.
- tmux truecolor: ~/.tmux.conf (default-terminal xterm-256color, truecolor, status off, mouse), ~/.zshrc
  + fish (n4dots) exports, backend tmux.py `new-session -e` (COLORTERM + CLAUDE_CODE_TMUX_TRUECOLOR).
- Integração do pull deles (multi-server aggregated session list / picker-scope / hide system-reminder)
  via `stash -u` → pull ff → pop (limpo) + fix do prop `servers` no CreateSessionSheet.

## Decisões
- Multi-PC cross-origin: app de UMA origem fala com backend de OUTRA. Cadeia: preflight (vite cors:false
  + apiCorsPreflight) → GET/POST (Bearer + CORS backend) → SSE (`?token`, backend lê). Mesmo túnel NÃO dá
  (hostname tailscale por PC); token pode ser igual (CP_AUTH_TOKEN). `set-environment -g` do tmux NÃO
  propaga pro pane → backend usa `new-session -e`.
- Desktop aditivo: media queries (min-width:820px) + DesktopShell; <820px = mobile byte-idêntico.
- Sidebar desktop usa getSessions() (server ativo) + switcher; a visão AGREGADA deles é a do mobile —
  convivem; dá pra unificar depois (api getAllSessions/AggSession).
- RESTART backend exige **cwd=backend** (`python -m app.main` precisa achar `app`). Relaunch correto:
  `( cd backend && CLAUDE_CONFIG_DIR=~/.claude-work setsid .venv/bin/python3 -m app.main >log 2>&1 & )`.

## Limitações conhecidas
- K (bloco preto no scroll iOS): 4 tentativas (removeu translateZ + guard no fit). Usuário parou de
  reportar após K#4 mas NÃO confirmou OK. Se voltar: tirar o overlap do glass.
- Desktop não 100% validado no PC (collapse da sidebar, largura, criar/trocar sessão). tmux fix só em
  sessão NOVA (existentes mantêm cor/TERM antigos).
- Dotfiles fora do repo claude-pocket: ~/.tmux.conf + ~/.zshrc (home), fish (n4dots, não-commitado lá).
- AskUserQuestion não surge no app (usar texto numerado). Uploads sem retention. send_prompt rejeita \n.

## Próximo passo
```
# 1. (FEITO) commit/push: desktop f0b7557 + tmux 7bca674. Backend restartado (pid 1458939).
# 2. Validar no PC: shell desktop (sidebar, collapse ☰, chat 920px), criar/trocar/apagar sessão, switcher.
# 3. tmux: criar sessão NOVA do app e ver as cores do claude certas (sem teal/pink) ao atachar no terminal.
# 4. Opcional: unificar a sidebar desktop com a visão AGREGADA multi-server (getAllSessions/AggSession).
# 5. Backlog: retention de uploads; AskUserQuestion no app; multi-linha (send_prompt rejeita \n).
# Stack: backend :8765 (cd backend && CLAUDE_CONFIG_DIR=~/.claude-work .venv/bin/python3 -m app.main),
#   vite :5173 --host (cors:false + apiCorsPreflight), tailscale serve → vite. Token em backend/.env.
#   Multi-PC: cada PC roda backend+vite+serve; mesma token; app guarda N servers e troca cross-origin.
# resume: /handoff resume (após git pull do projeto E do repo de skills)
```

## Arquivos criticos
- Backend (N): app/workflows.py (parser dos runs de workflow). (R): app/{api.py (endpoints /workflows + CORS), auth.py (?token), tmux.py (-e truecolor), transcript.py (filtros meta), pqueue.py (fila durável)}.
- Frontend desktop (N): components/{Sidebar,DesktopShell}.svelte. (R): App.svelte (branch isDesktop), MessageList/Composer (media query 920).
- Frontend atividade (N): lib/activity.ts, components/ActivitySheet.svelte (drill-in 3 níveis).
- Frontend multi-PC (R): lib/auth.ts (multi-server), lib/api.ts (getAllSessions + getWorkflow*), vite.config.ts (cors:false + apiCorsPreflight).
- Config tmux (fora do repo): ~/.tmux.conf; doc no repo: docs/tmux-truecolor-setup.md.
