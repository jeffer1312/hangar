---
branch: main
saved_at: 2026-06-26T06:55:28-03:00
saved_commit: 77ab21080b4dcc1817a503b1b93ef35ec4f4fa35
status: in_progress
---


## TL;DR
claude-pocket (dirigir Claude Code do celular, dogfooding). Sessão grande de polish + features, tudo
buildando (svelte-check 0 erros, vite build OK). Entregue: glass iOS, scroll dinâmico anti-glitch,
multi-imagem, merge stop/send, ImageBubble (miniatura+lightbox portal), **fila durável no backend**
(sidecar + 3ª fonte no SSE, dedup vs transcript, ordem por ts), filtro de inject de skill, **fila visual
(atenua=na fila / acende=aceita)**, **multi-servidor/multi-PC (M)** e **K#3** (anti-preto). Backend
REINICIADO 2x (fila + CORS ativos; CORS preflight OPTIONS=200 ACAO=*). Falta VERIFICAR no aparelho:
K (preto no scroll, 3ª tentativa = desliga glass no scroll) e M cross-origin (server do trabalho).

## Task atual
Polish + features via dogfooding. Código todo buildando. Aguardando confirmação no aparelho de: (1) K
(bloco preto no scroll) resolvido; (2) fila durável (persiste + atenua/acende). Próxima feature pedida:
multi-PC no app (casa + trabalho, mesma token, URLs tailscale diferentes).

## Concluído nesta sessão
(diff: 8 arquivos M + pqueue.py N; ~353 inserções. Não commitado ainda no momento deste save.)
- Glass iOS no Composer (blur 30 + vibrancy + specular hairline + radius 18).
- Scroll: padding da lista = altura REAL do dock (ResizeObserver), keyboard-safe (não dispara na anim).
- Multi-imagem: composer N anexos, protocolo 1-linha "📎 imagem: p1 📎 imagem: p2", parser N, galeria.
- Merge stop/send: 1 slot (working+vazio=stop; digitou/colou=send).
- ImageBubble: miniatura no topo, legenda embaixo, clique=lightbox (portal pro <body>, escapa transform).
- Pending/queued à direita (flex-end) — não cola na esquerda (cara de assistente).
- Fila: reconcile por linha → solidify → **FILA DURÁVEL backend** (pqueue.py). Visual: atenua na fila,
  acende no idle (= aceita). Persiste no reload.
- Filtro inject skill (transcript.py dropa "Base directory for this skill:").
- Bug K (preto scroll iOS): transform só com offsetTop>0; removeu -webkit-overflow-scrolling; bg sólido;
  translateZ no scroller.
- Backend reiniciado (pid novo, :8765) — fila durável no ar.
- Fila VISUAL: msg da fila (`queued-`) atenua enquanto working, acende sólida no idle (= aceita).
- **M — multi-servidor/multi-PC**: auth.ts vira lista de servers (id/label/baseUrl ABSOLUTO/token) +
  ativo; migra o single-server; addServer/selectServer/removeServer/dropActiveServer. Login+SessionList
  usam a API nova (baseUrl = origin absoluto). Switcher no menu da SessionList (lista + ✓ ativo + QR add
  + remover). 401 dropa só o server ativo. Backend ganhou **CORS** (allow *, token-gated) pro app de uma
  origem falar com backend de outra (API Bearer + SSE ?token). Trocar de server = window.location.reload.
- **K#3** (preto no scroll, 3ª tentativa): além de transform-só-offsetTop>0 + bg sólido + translateZ, o
  Composer desliga o `backdrop-filter` do glass DURANTE o scroll (Chat detecta via onScrollActivity +
  timer 160ms) — o backdrop re-amostrando conteúdo em movimento era o suspeito nº1.

## Decisões
- Fila NÃO injeta no transcript do Claude Code (ele é dono: race/corrupção, double-write). Sidecar próprio
  + merge na LEITURA.
- Msg enfileirada (sent while working) NEM sempre vira entrada no transcript do Claude Code — só idle-sends
  viram prompt gravado (descoberto comparando o transcript vivo b9b88f0b). Por isso a fila própria é o
  registro real; sintético dedup-a contra o transcript quando o Claude grava.
- Queue grava só texto não-"/" (comandos são meta, não viram bubble).
- ts (campo ISO `timestamp` do transcript) usado SÓ pra ordenar no merge de history; NÃO exibido
  (ChatEvent.ts fica None — senão bubble enfileirada mostraria hora e as do transcript não).
- RESTART backend: 2 armadilhas. (1) `pkill -f app.main` CASA O PRÓPRIO SHELL (o texto do script contém
  "app.main") → mata a si mesmo. (2) `SIGTERM` TRAVA: o SSE do celular segura a conexão e o uvicorn não
  encerra → a porta fica presa e o relaunch morre (Address already in use). SOLUÇÃO: pegar o pid pela porta
  (`ss ... grep :8765 ... pid=`), `kill -9` (libera o listen socket na hora), relaunch detached (setsid) +
  `curl --retry --retry-connrefused` pra esperar subir (sem `sleep` de shell).
- M cross-origin: cada server guarda baseUrl ABSOLUTO (origin) — '' (same-origin) era ambíguo entre PCs.
  CORS no backend é o que permite o app de uma origem falar com backend de outra. Mesmo túnel NÃO dá (cada
  PC tem hostname tailscale próprio); token pode ser a mesma (CP_AUTH_TOKEN igual).

## Limitações conhecidas
- Bug K não verificado no aparelho (2 tentativas). Se persistir: desligar overlap/backdrop-filter do glass
  durante o scroll (ou só no momentum).
- Fila durável: dedup por texto pode over-dedup se o user mandar texto idêntico 2x. Aceitável.
- Backend SEM --reload: toda mudança no backend exige restart manual.
- App é single-server (1 baseUrl+token). Pareou outro PC = sobrescreve. Pedido M (multi-PC) ainda não feito.
- AskUserQuestion não surge no app (usar texto numerado). send_prompt rejeita \n (multi-linha real: backlog).
- Uploads acumulam (sem retention). Paste de imagem instável no iOS.

## Próximo passo
```
# 1. (FEITO neste turno) handoff save + commit + push.
# 2. VERIFICAR no aparelho: K#3 (scroll sem preto? glass desliga no scroll), fila (atenua→acende +
#    persiste no reload), multi-imagem, lightbox.
# 3. M cross-origin no TRABALHO: lá, `git pull` + `CP_AUTH_TOKEN` IGUAL ao de casa em backend/.env +
#    subir backend + `tailscale serve`. No app (carregado de casa): menu > "+ Adicionar servidor" > QR do
#    trabalho. Deve listar os 2 e trocar (✓). Se a chamada cross-origin falhar, conferir CORS (já ativo).
# 4. Se K#3 persistir: último recurso = remover o overlap do glass (dock flex, lista acima dele).
# Stack: backend :8765 (uv/.venv, CLAUDE_CONFIG_DIR=~/.claude-work, token em backend/.env), vite :5173
#   --host, tailscale serve (https://jefferson-felizardo.tailcac351.ts.net/?token=...).
#   Restart backend: kill <pid> (NUNCA pkill -f app.main) + ( cd backend && CLAUDE_CONFIG_DIR=~/.claude-work
#   setsid .venv/bin/python3 -m app.main > log 2>&1 & ) + curl --retry pra confirmar :8765.
# resume: /handoff resume (após git pull)
```

## Arquivos criticos
- Backend fila (N): backend/app/pqueue.py — PromptQueue (sidecar) + merged_history (ordem por ts + dedup linha).
- Backend (R): backend/app/{api.py (history→merged_history; input grava fila; CORS), sse.py (3ª fonte), transcript.py (filtro skill)}.
- Frontend multi-PC (R): lib/auth.ts (REESCRITO: lista de servers + ativo + migração), lib/api.ts (401→dropActiveServer), screens/Login.svelte (addServer + baseUrl origin), screens/SessionList.svelte (switcher de servers no menu + QR add).
- Frontend fila/scroll/glitch (R): screens/Chat.svelte (dockH ResizeObserver, reconcile por linha, solidify, dedup cruzado no SSE, transform só offsetTop>0, bg sólido), components/MessageList.svelte (padding dinâmico, queued-row atenua/acende, pending flex-end, bg+translateZ anti-preto).
- Frontend visual (R): components/Composer.svelte (glass iOS, multi-anexo, merge stop/send), components/ImageBubble.svelte (miniatura+lightbox portal), lib/format.ts (parse N imagens).
