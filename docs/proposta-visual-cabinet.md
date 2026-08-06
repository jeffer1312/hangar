# Proposta: o que extrair do Cabinet pro claude-cockpit

Fonte: github.com/cabinetai/cabinet (cloned em /tmp/pi-github-repos/cabinetai/cabinet).
Data: 2026-08-25. Autor: Claude.

O que faz o Cabinet parecer bonito não é nenhuma peça única — é uma decisão
arquitetural que eles chamam de **"Manila Arc"**: a página inteira é uma
"mesa" (gutter), e por cima dela flutua uma "folha" (sheet) com cantos
arredondados e sombra suave. Tudo o que é chrome (toolbar, breadcrumb,
statusbar) fica FORA da folha, na mesa.

## 1. A ideia central: desk + sheet (a que vale mais)

Hoje o cockpit é "cheio" de ponta a ponta: sidebar cola na borda, chat cola
na sidebar, tudo se separa por borda fina. O Cabinet faz ao contrário:

- O fundo da janela inteira é o **gutter** (mesa) — cor derivada da base com
  um mix: `color-mix(in oklch, var(--background) 90%, var(--foreground) 10%)`.
  No dark vira uma cor um pouco mais escura que o fundo (90→85+black).
- Por cima, a área de conteúdo é uma **sheet** com:
  - `--sheet-radius: var(--radius-2xl)` (raio grande, segue a escala do tema)
  - `--sheet-shadow`: no light `0 10px 28px -6px rgb(0 0 0 / 0.14)`; no dark
    `inset 0 1px 0 rgb(255 255 255 / 0.05), 0 12px 30px -6px rgb(0 0 0 / 0.55)`
    — o `inset 0 1px` claro é o truque que "levanta" a folha no escuro sem
    precisar de borda visível.
  - Margem de ~10px em cima, direita e baixo (esquerda fica a sidebar flush).
- Sidebar fica colada na esquerda SEM borda, separada pela sombra da folha.
- Statusbar fica embaixo DA folha, na mesa (não dentro da folha).
- **html/body com `overflow: hidden`** — a mesa nunca dá scroll; só o rail da
  sidebar e a folha fazem scroll interno. Isso tira o "balanço" de página web
  e trata o app como objeto sólido.

Custo no cockpit: wrapper no shell + ~15 tokens CSS novos. O efeito visual é
desproporcional ao esforço — é o único item que muda a "cara" do app inteiro.

## 2. Bordas sussurradas (o que mata o visual "caixoteado")

O comentário deles no globals.css é literal: "neutral hairlines gave the app
its old boxy look". A solução:

- O `--border` global vira sussurro no light:
  `--border-soft: color-mix(in oklch, var(--border) 22%, transparent)` —
  22% de opacidade da borda que antes era sólida. Superfícies se separam por
  preenchimento + elevação, não por linha.
- No dark NÃO suaviza mais (a borda dark deles já é `white 10%` — suavizar
  apagaria de vez).
- Cards, que flutuam sobre a folha clara, ganham uma borda MÉDIA:
  `--card-edge: color-mix(in oklch, var(--border) 55%, transparent)`.
  Leitura: borda normal para a página, borda um pouco mais firme para card.

Mapeamento no cockpit: o `--border-subtle` (7%) e `--border-default` (12%)
já são bem suaves. O ganho real seria aplicar a hierarquia de três níveis
(página sussurrada / card médio / input intacto) de forma sistemática em vez
de componente a componente.

## 3. `--terminal-*`: paleta ANSI derivada do tema

O terminal de web deles não tem cores hardcoded: as 16 cores ANSI + bg, fg,
cursor e selection são todas `color-mix()` A PARTIR dos tokens do tema
(--primary, --destructive, --background). Trocou de tema, o terminal
acompanha. Nosso xterm provavelmente tem uma paleta fixa hoje — daria pra
gerar `--terminal-ansi-*` no app.css e alimentar o tema do xterm via JS
lendo as CSS vars.

Prioridade: baixa a média — vale quando trocar de tema estiver valendo.

## 4. Tokens utilitários pequenos que a gente pode copiar

- Escala de raio derivada de `--radius` base:
  `--radius-sm/md/lg/xl/2xl/3xl/4xl` via `calc(var(--radius) * fator)`.
  Um único lugar muda a "curvatura" do app inteiro. (Cockpit hoje: raios
  hardcoded.)
- Tokens de card/sidebar separados do resto (`--card`, `--sidebar`,
  `--sidebar-accent`...) — sistema de 14+ slots em vez de 4 backgrounds.
- `:focus-visible` com outline explícito (2px solid var(--ring), offset 2px)
  — acessibilidade uniforme sem depender dos defaults de cada controle.

## 5. Avatares com identidade (o toque de personalidade)

`public/agent-avatars/avatar-01.svg … avatar-112.svg`: 112 avatares SVG
prontos, atribuídos por hash do nome/slug do agente. No cockpit, cada sessão
poderia ganhar um avatar determinístico do session-id — resolve a "parede de
texto" da lista de sessões e dá cara pro pareamento (claude-pocket). Custo:
copiar os SVGs (ou gerar versão própria) + 10 linhas de hash→índice.

## 6. Home/dashboard que a gente não tem (e eles têm)

A home do Cabinet é um hub com: quick actions (chips de tarefa comum), card
de agentes com TiltCard, scheduler/recents, empty states bonitos. O cockpit
abre direto no chat. Se um dia a gente quiser uma "tela inicial" (sessões
ativas, marcos recentes, chips de ação "retomar sessão X"), o home-screen.tsx
deles é o modelo de informação — mas isso é feature nova, não polimento.

## 7. O que NÃO trazer

- **112 temas custom + fontes por tema** (`data-custom-theme`, `--font-theme`
  com overrides CJK): complexidade enorme, cockpit tem 2 temas e set de
  tipografia já bom.
- **Sistema de rooms/cabinets/settings deles**: Next.js + Zustand; cockpit é
  Svelte/TS puro. Portar o conceito, nunca o código.
- i18n/RTL deles: fora do nosso escopo.

## Resumo do ganho por esforço

| Ideia | Esforço no cockpit | Efeito visual |
|---|---|---|
| Desk + sheet (electron-style) | 1-2h (shell + tokens) | alto — a cara muda |
| Bordas sussurradas hierárquicas | 1h (retoque de tokens + auditoria) | médio-alto |
| Raio derivado de --radius | 30min | médio |
| Avatar por sessão | 1h + assets | médio (identidade) |
| Terminal ANSI temático | 2h (JS lendo vars) | baixo |
| Home/dashboard | dias | feature nova, não polimento |

## Referências no código do Cabinet

- globals.css: src/app/globals.css (tokens, comentários "#086/#089/#090/#100"
  explicando cada decisão da Manila Arc)
- Sheet: src/components/layout/content-sheet.tsx
- Shell/gutter: src/components/layout/app-shell.tsx (~linhas 1100-1170)
- Home: src/components/home/home-screen.tsx
- Avatares: public/agent-avatars/
