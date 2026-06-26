---
branch: main
saved_at: 2026-06-25T23:40:00-03:00
saved_commit: 36d9e4889ba846b152efe1dec56a40cdae6d0ea9
status: in_progress
---

## TL;DR
claude-pocket (dirigir Claude Code do celular). Maratona de dogfooding. Já entregue e funcionando:
backend+frontend core, Plan 3, redesign, **Phase 4** (métricas), **upload+preview de imagem**, fixes
de confiabilidade (401 self-heal, 500-fix, feedback de erro). Esta leva = POLISH visual: assistente
**sem bubble** + **tool calls colapsados** (cara Claude iOS), **ring dentro do pill** (% no círculo),
**cwd+branch** no topo do composer, e o **glass**. O glass de OVERLAP (desfocar a lista atrás) **briga
com o teclado iOS** — quebrou 2x. HEAD (36d9e48) = tentativa estabilizada do overlap, **NÃO verificada
no aparelho**. PRÓXIMO grande: **R&D de abordagem estável pro glass de verdade**. ~7 commits locais (pushados). Stack roda.

## Task atual
Glass: o usuário gostou do composer FLUTUANTE por cima da lista (blur real). Mas overlap (dock absoluto)
glitcha o teclado iOS. HEAD=36d9e48 é um retry estabilizado — TESTAR no aparelho. Se glitchar, fallback:
`git revert 36d9e48` volta pro glass estável SEM overlap (b0c47dd: card fosco translúcido, dock flex).

## Concluído nesta sessão
Polish visual (git 0b0ba7a..HEAD):
- Assistente sem bubble + tool calls colapsados "Executou X ›" (062b209).
- Composer: `/` foi pro topo (linha do preço); ContextRing DENTRO do pill do modelo, % no círculo (1f39b76).
- cwd + branch no topo do composer: `📁 repo · branch` (* âmbar se sujo), parseado da statusline (6fcc7ea).
- Glass v1 overlap (13fa581) → glitch teclado; "fix" hard-lock+sem-translateY (7f81093) → pior (composer
  voou pro topo); REVERT dos 2 + glass card estável sem overlap (b0c47dd); RETRY overlap estabilizado (36d9e48).
- (Sessões anteriores: upload+preview de imagem, 500-fix, feedback, keyboard viewport-flex — ver git log.)

## Decisões
- **Teclado iOS que FUNCIONA** (confirmado pelo user no 48d52e3): `.chat-screen` flex col, height JS =
  `visualViewport.height` + `transform: translateY(vv.offsetTop)`, **dock como FLEX item**, NavBar flex topo.
  NÃO mexer nisso.
- **Overlap-glass quebra o teclado**: dock `position:absolute` + o transform do container glitcha na animação
  (NavBar some / fragmento do composer no topo). Remover o translateY → composer voa pro topo (offsetTop
  não compensa). `bind:clientHeight`/`--dock-h` dinâmico reflowando na animação = suspeito do glitch.
- 36d9e48 tenta: dock absoluto + padding FIXO da lista (`calc(150px + safe-area)`, sem --dock-h) + guard
  `if (vv.height < 120) return` no fit. Pode não bastar.
- Glass card = `rgba(24,24,27,.6)` + `backdrop-filter: blur(22px) saturate(180%)` + border branco translúcido + shadow.

## Limitações conhecidas
- **Glass de verdade (overlap) instável no iOS** — precisa R&D de outra abordagem (barras fixas ancoradas na
  visualViewport SEM transform do container; ou outra arquitetura). HEAD não verificado.
- **AskUserQuestion NÃO surge no app** (debug pendente) — com user usar TEXTO NUMERADO.
- **Sem fila durável** no servidor. Multi-linha de texto não suportada (send_prompt rejeita `\n` → 400).
- Uploads acumulam (falta retention). Paste de imagem instável no iOS (usar 📎).

## Erros / armadilhas
- NÃO remover o `translateY(vv.offsetTop)` do fit() — necessário (sem ele o composer voa pro topo).
- Restart backend: `pkill -f app.main` + relaunch na mesma linha teve RACE; usar `kill <pid>` + guard `ss|grep :8765`.
- send_prompt rejeita control chars → input_prompt 400 (não 500). Mensagem de imagem é UMA LINHA (sem `\n`).
- backend/.env = token estável (gitignored); test_config hermético (public_url="").

## Arquivos criticos
- Frontend layout/glass (R): screens/Chat.svelte (fit() height+translateY+guard; dock absoluto), components/MessageList.svelte (padding fixo), components/Composer.svelte (glass card + cwd/branch + ring no pill).
- Frontend visual (N): components/{ContextRing,AssistantBubble,ToolCard,ImageBubble}.svelte; lib/{statusline.ts,format.ts}.
- Backend: app/{uploads.py,api.py,state.py}; backend/.env. Docs: future-features.md, plans/specs.

## Próximo passo
```
# 1. (FEITO neste save) commit handoff + push.
# 2. TESTAR 36d9e48 no aparelho (teclado + glass overlap). Glitchou -> git revert 36d9e48 (volta ao b0c47dd estável).
# 3. R&D glass real: barras ancoradas na visualViewport SEM transform do container (position:fixed top=offsetTop/height=vv.height),
#    ou arquitetura que permita overlap estável. Iterar com screenshots.
# 4. Fila: contenteditable (barra teclado iOS); fila durável servidor; git-ops (feature); filtrar meta-options; debug AskUserQuestion-app; retention uploads.
# Stack: backend :8765 (token B_cCngF3YyM31J3CAOMMK9-e em .env), vite :5173 --host, tailscale serve.
# resume: /handoff resume (após git pull)
```
