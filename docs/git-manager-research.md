# Git Manager — pesquisa de layout (brief + fontes)

> Gerado por workflow multi-agente em 2026-07-03. Alimenta o plano em `docs/superpowers/plans/2026-07-03-git-manager.md`.

# BRIEF DE DESIGN — Git Manager do claude-pocket

Base verificada no código (âncoras reais):
- **Push-view já existe**: `GitSheet.svelte:146-163` — `{#if diffPath}` troca a sheet inteira pelo diff viewer com botão `‹ voltar`. É o mecanismo a reusar pra log e pra detalhe de commit.
- **Log hoje NÃO tem view própria**: o botão `log` (`:171`) chama `runAction('log')`, que despeja `%h  %s  (%an, %ar)` no `<pre class="git-output">` genérico do rodapé (`:243`). Backend: `git_ops.py:91`, dentro do dict estático `_ACTIONS`, retornado como string crua por `git_action` (`:95`).
- **Lista de arquivos**: `.git-file` = `.git-file-tag` + `.git-name` (`:201-204`). Hoje `.git-name` é `nowrap; overflow:hidden; text-overflow:ellipsis` (`:329`) → trunca no FIM, escondendo justamente o basename. É o bug que D corrige.
- **Diff render**: `<pre class="git-diff">` com `<span>` por linha classificado `add/del/hunk/meta` (`:156-161`) — os tokens do Shiki entram DENTRO desses spans.
- **Resize handle pronto pra copiar**: `Sidebar.svelte` — `width=$state(clamp(localStorage 'cp_sidebar_w'))` (`:44`), `resizeStart/Move/End` com `setPointerCapture` (`:46-57`), persiste só no `resizeEnd`, `.resize-handle { cursor:col-resize; touch-action:none }` (`:530,659`).
- **Nenhuma lib de highlight instalada** (só `lottie-web`, `qr-scanner`) → Shiki é dep nova.

---

## Mudança de backend transversal (habilita A, B e F) — fazer UMA vez

Trocar o `log` estático por uma função dedicada tipada (mesmo padrão de `file_diff` `:124`), com formato delimitado por bytes de controle e **campos superset** que servem os três itens de uma vez — não remodelar depois:

```
git -C <cwd> log --topo-order -n 50 \
  --pretty=format:%H%x1f%h%x1f%P%x1f%D%x1f%an%x1f%at%x1f%ar%x1f%s%x1e
```

`%H` (hash completo — âncora de aresta do grafo E lookup de detalhe), `%h` (curto, exibição), `%P` (parents — custo zero agora, evita troca de formato na fase F), `%D` (refs sem parênteses), `%an`, `%at` (unix, ordenação estável), `%ar` (relativo pronto), `%s`. Split em Python: `raw.strip('\x1e').split('\x1e')` → cada `.split('\x1f')`. `--topo-order` obrigatório pra grafo não intercalar branches por data.

Endpoint novo (não reusar o `git_action` genérico de string). A **atribuição de lanes** (fase F) roda depois, como passo extra sobre esses mesmos dados — não muda o formato.

**Esforço: P-M. Risco: baixo** (parsing trivial; `%x1f/%x1e` nunca aparecem em texto de commit).

---

## A. View de LOG dedicada (ocupa o sheet como o diff)

**Abordagem**: adicionar um ramo de view irmão do `{#if diffPath}`. Como agora há três destinos empilháveis (lista → log → detalhe-de-commit) **além** de lista → diff, trocar o par de booleanos por um enum de view: `type GitView = 'list' | 'log' | 'diff' | 'commit'`, `let view = $state<GitView>('list')`. O `‹ voltar` decide o alvo pelo enum. Consome o endpoint estruturado acima via um método novo em `api.ts` (espelha `getFileDiff`).

**Lib**: nenhuma — reuso do padrão já no arquivo.
**Esforço: M** (endpoint + método api + ramo de view + refactor booleano→enum).
**Risco: baixo**. O único cuidado é não regredir o fluxo do diff ao trocar `diffPath` por `view`.

## B. Log = UMA LINHA POR COMMIT (estilo TortoiseGit)

**Abordagem**: renderiza dentro da view A. Layout (R1, já com CSS pronto): `[dot/iniciais 18px] [%h mono 7ch fixo] [ref pill se %D] [subject flex ellipsis] [%ar curto à direita]`. **Ellipsis no subject, NÃO scroll horizontal** (scroll-x por linha briga com o scroll vertical da lista de toque; e a convenção de 50 chars do git faz truncar raramente). Nome completo do autor, e-mail, hash full, data absoluta e corpo vão pro **detalhe ao tocar** — que é a própria view `'commit'` do enum (push-view de novo, sem accordion inline → sem reflow de altura).

Cor do dot = hash determinístico do autor; pill de ref colorida por tipo (local verde / remote vermelho / tag amarelo), só renderiza se `%D` existir.

**Lib**: nenhuma.
**Esforço: P** (a estrutura de dados vem de A; é CSS + `{#each}`).
**Risco: baixo**. Depende inteiramente de A estar pronto.

## C. Syntax highlighting no diff (e opcionalmente no log) — estilo VSCode

**Abordagem/lib**: **Shiki**, modo *fine-grained bundle* (`shiki/core`) + **engine JS** (`createJavaScriptRegexEngine`, sem WASM) + import dinâmico por linguagem.

**Por que atende no-CDN/offline/mobile**: 100% ESM, zero fetch remoto (herança do Shikiji "no more CDN"); temas `dark-plus`/`light-plus` são os arquivos reais do VS Code (fidelidade que hljs/Prism não têm); o engine JS descarta o Oniguruma WASM (~1.5MB → ~4%), viável em LAN/celular. Casa direto com o toggle `data-theme` do app.

**Integração** (R2, recipe validado): singleton em `lib/highlight.ts`, `createHighlighterCore` async **uma vez** (o `diffLoading` já cobre a espera), `codeToTokensBase` **síncrono** depois. **Gotcha crítico**: grammars TextMate são stateful entre linhas → tokenizar o **blob de código do hunk inteiro** (sem os prefixos `+`/`-`/espaço) de uma vez e recasar os tokens linha-a-linha; NUNCA linha isolada (quebra string/comentário multi-linha). Os tokens entram DENTRO dos `<span class:add/del>` existentes (`:156-161`) — mantém 100% do CSS de fundo atual. Só as linguagens do repo (ts/tsx/js/svelte/py/sh/json/yaml/md/css/html) no mapa de loaders; extensão desconhecida → plain text (não quebra). No log, opcional e mais barato: `lang:'diff'`.

**Esforço: M**. **Risco: médio** — é a maior incógnita: peso dos grammars (mitigado por dynamic-import + JS engine), init async (já ok), e o gotcha stateful (endereçado pelo recipe). Medir o bundle isolado após integrar.

## D. Lista de ARQUIVOS ALTERADOS — basename em destaque

**Abordagem**: quebrar `.git-name` (`:203`) em dois: `<span class="dir">` + `<span class="base">`. Truncar **no MEIO/início do dir**, nunca o basename (hoje faz o contrário). Truque CSS-only, sem JS de medição (R4): dir = `flex-shrink:1; overflow:hidden; text-overflow:ellipsis; direction:rtl` (reticência aparece no começo, mantém o fim visível), base = `flex-shrink:0`, peso/cor de destaque; dir com opacidade/tamanho menor. Split do path: `path.split('/')`, último = base, resto = dir. Reaproveitável na lista de arquivos do detalhe-de-commit (item B).

**Lib**: nenhuma (CSS puro).
**Esforço: P** (o menor de todos). **Risco: ~nulo**.

## E. Sheet REDIMENSIONÁVEL (drag handle)

**Abordagem desktop**: copiar o padrão do `Sidebar` verbatim — `resizeStart/Move/End` + `setPointerCapture` + persistência só no `pointerup` em `localStorage 'cp_gitsheet_w'` + `.resize-handle` (`touch-action:none`, `cursor:col-resize`, hit-area folgada). Aplica quando o `GitSheet` está docado como painel lateral (`@media min-width:820px`).

**Mobile** (sheet, não painel): resize **vertical por detents** (alturas fixas) com **grabber** no topo — arrastável e tap-cicla altura; área de toque ~48px (não só a barrinha). NN/g: manter o botão de fechar explícito (gesto de arraste colide com gestos do sistema).

**Esforço: P (desktop, é cópia) / M (mobile detents)**. **Risco: médio de escopo** — `GitSheet` usa o `BottomSheet` **compartilhado** (AskQuestionSheet etc.). **Verificar antes** se dá pra escopar o resize só ao GitSheet (prop opt-in no `BottomSheet` ou wrapper), pra não afetar as outras sheets. Desktop primeiro; detents mobile podem ser fase separada.

## F. COMMIT GRAPH visual — Fase 2

**Abordagem/lib**: **sem dependência** (`@gitgraph/js` está arquivado e é API imperativa, não consome `git log` real; `dolthub/commit-graph` é React sem porte Svelte). Algoritmo de lanes (~30-50 linhas, R3) roda **no backend Python** sobre os dados já estruturados (parents `%P` já vêm do endpoint de A): reuso de slot livre (`None`) em vez de sempre `append`; 1º parent = branch child (aresta reta, mesma coluna); parents extras = merge (aresta curva). Emite `[{hash, col, parents:[{col,curved}]}]`. Frontend desenha `<svg>` trivial (dots + `<path>` bezier) — **zero JS de grafo no client, self-contained, nítido em qualquer DPI, hit-test por dot de graça, sem redraw no toggle de tema**.

**Sheet estreito** (R1): degradar, não replicar — cap ~4-5 lanes, gutter do grafo com `overflow-x` independente da coluna de texto; abaixo disso, rail linear fino (CSS `::before`) + merge vira **glifo inline** (`⑂`) na linha, não lane extra. Multi-lane completo só no dock desktop `≥820px`.

**Esforço: G**. **Risco: médio** — correção do algoritmo de lanes (deixar UM `demo()`/assert cobrindo linear + 1 merge + 1 branch nova, R3) e legibilidade em sheet estreito (mitigada pelo degrade).

---

## ORDEM DE IMPLEMENTAÇÃO

1. **D (file list basename)** — CSS puro, zero dep, risco nulo, ganho visível imediato e isolado. Aquecimento.
2. **Endpoint de log estruturado** (superset com `%P`) — desbloqueia A/B/F. Feito uma vez, certo, com os parents já incluídos pra não trocar formato depois.
3. **A + B juntos** (view de log + linha única + refactor view-enum + detalhe-ao-tocar) — é uma feature só e entrega a UX headline pedida.
4. **E desktop** (resize por cópia do Sidebar) — barato, melhora ler log/diff no dock. (Detents mobile: fase à parte.)
5. **C (Shiki)** — maior dep/risco, é decoração viewer-only, não bloqueia nada. Fazer sobre uma estrutura já pronta, medindo o bundle isolado.
6. **F (commit graph)** — Fase 2, o maior, exige o algoritmo de lanes + teste; desktop-first.

**Racional**: primeiro o mais barato/seguro (D) e o desbloqueador de dependência (endpoint), depois a feature principal (A+B), depois ergonomia (E), e por último os dois pesados (C, F) — ambos são aditivos sobre uma estrutura já funcionando e carregam o risco real de bundle/complexidade. Isolados no fim, uma regressão de bundle (C) ou um bug de lane (F) não travam o log core de sair.


---

# Apêndice — relatórios de pesquisa (fontes)


## R1 — UI de histórico de commits

# Redesign do commit log no GitSheet — pesquisa + recomendação

Contexto do código atual: `GitSheet.svelte` já tem o padrão de "push-view" (lista → detalhe, com botão `‹ voltar`) usado no diff viewer (`diffPath`, linhas 146-163). O botão "log" hoje só joga a saída crua de `git log --pretty=format:"%h  %s  (%an, %ar)"` (backend `git_ops.py:91`) dentro de um `<pre class="git-output">` com `white-space: pre-wrap; word-break: break-all` — por isso o wrap feio. A recomendação abaixo reaproveita o mesmo padrão visual/estrutural já existente (não introduz um componente novo com convenções diferentes).

## 1. Linha única por commit — layout concreto

**Campos, ordem e prioridade** (do que sobrevive em 320-380px de sheet estreito):

```
[●/av] a1b2c3d  Corrige race condition no polling do SSE          2h
        └hash┘  └────────── subject (flex, ellipsis) ──────────┘ └rel┘
```

| Campo | Fonte | Estilo | Comportamento |
|---|---|---|---|
| Marcador de autor | avatar/iniciais **ou** dot colorido | círculo 18-20px, `flex-shrink:0` | substitui o nome do autor por extenso — GitLens faz exatamente isso quando a coluna aperta: "when resized to minimum width, [Author column] shows avatars instead of text… columns that become too narrow automatically switch to icons to preserve information" ([GitLens Commit Graph](https://help.gitkraken.com/gitlens/gl-commit-graph/)) |
| Hash curto | `%h` | `font-family: var(--font-mono); color: var(--text-muted); font-size: var(--text-xs)` largura fixa (7ch) | nunca trunca, é a âncora visual monoespaçada — é o padrão canônico do próprio `--oneline` do git ("condenses each commit to a single line, displaying only the commit ID and the first line of the commit message", [git-log docs](https://git-scm.com/docs/git-log)) |
| Subject | `%s` | `flex:1; min-width:0; white-space:nowrap; overflow:hidden; text-overflow:ellipsis` | **ellipsis, não scroll horizontal.** Raramente vai truncar de verdade: a convenção do próprio Git já limita a subject line a ~50 caracteres ("the first line of the commit message should be 50 characters or less" — convenção citada inclusive nas discussões de UI do Sourcetree). Scroll horizontal por linha é ruim em lista vertical de toque (conflita com o scroll da lista); ellipsis + tap-to-expand é o padrão que todo cliente mobile usa |
| Data relativa | `%ar` | `color: var(--text-muted); font-size: var(--text-xs)`, largura fixa curta (`2h`, `3d`, `1sem`), alinhado à direita | forma curta agressiva — inspirado no componente oficial do GitHub `<relative-time>`, que formata para o formato relativo mais curto que cabe ("values rounded to display a single unit", [github/relative-time-element](https://github.com/github/relative-time-element)) |
| Refs (branch/tag) | `%D` | pill pequena antes do subject, cor por tipo (local=verde, remoto=vermelho/laranja, tag=amarelo) — **só renderiza se existir** | reproduz a semântica de cores do `--decorate` nativo do git: "remotes are in red, HEAD is blue, local branches are in green, stashes in pink" ([explainshell: git log --oneline --graph --decorate](https://explainshell.com/explain?cmd=git+log+--oneline+--graph+--decorate+--all)) |

**O que NÃO cabe na linha e vai pro detalhe ao tocar:** nome completo do autor, e-mail, hash completo, data absoluta, corpo do commit (`%b`), e lista de arquivos alterados desse commit.

**Interação tap-to-expand**: reusa o padrão já existente de "push-view" do diff (`diffPath` → troca o conteúdo da sheet inteira, com back-button), em vez de accordion inline — evita reflow de altura da lista e é consistente com o resto do app. É também como TortoiseGit e Working Copy resolvem isso: "the log dialog... top pane [list] / middle pane shows the full log message for the selected revision / bottom pane shows list of files changed" ([TortoiseGit Log Dialog](https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html)); Working Copy: "each commit contains the message, author, date, sha1 identifier..." na lista, e toque/long-press abre ações e detalhe ([Working Copy manual](https://workingcopy.app/manual/repository-sheet/)).

### Svelte (drop-in, mesma convenção de classes `git-*` do arquivo)

```svelte
<!-- dentro do git-scroll, substituindo o <pre class="git-output"> pro log -->
{#if commits.length}
  <div class="git-commits">
    {#each commits as c (c.hash)}
      <button class="git-commit" onclick={() => openCommit(c.hash)}>
        <span class="git-commit-dot" style:background={authorColor(c.author)} aria-hidden="true">
          {initials(c.author)}
        </span>
        <span class="git-commit-hash">{c.hash}</span>
        {#if c.refs.length}
          <span class="git-ref" data-t={c.refs[0].type}>{c.refs[0].name}</span>
        {/if}
        <span class="git-commit-subject">{c.subject}</span>
        <span class="git-commit-time">{c.relDate}</span>
      </button>
    {/each}
  </div>
{/if}
```

```css
.git-commits { display: flex; flex-direction: column; gap: 1px; }
.git-commit {
  display: flex; align-items: center; gap: var(--space-2); width: 100%;
  padding: var(--space-2); border-radius: var(--radius-md);
  border: 1px solid transparent; background: transparent;
  color: var(--text-secondary); font-size: var(--text-sm); text-align: left; cursor: pointer;
}
.git-commit-dot {
  flex-shrink: 0; width: 18px; height: 18px; border-radius: 50%;
  display: flex; align-items: center; justify-content: center;
  font-size: 9px; font-weight: 600; color: #fff;
}
.git-commit-hash {
  flex-shrink: 0; font-family: var(--font-mono); font-size: var(--text-xs); color: var(--text-muted);
}
.git-ref {
  flex-shrink: 0; font-size: 10px; font-family: var(--font-mono); padding: 0 4px; border-radius: 3px;
}
.git-ref[data-t="local"]  { color: #4ec9b0; background: color-mix(in srgb, #4ec9b0 15%, transparent); }
.git-ref[data-t="remote"] { color: #f07178; background: color-mix(in srgb, #f07178 15%, transparent); }
.git-ref[data-t="tag"]    { color: #d9a441; background: color-mix(in srgb, #d9a441 15%, transparent); }
.git-commit-subject {
  flex: 1; min-width: 0; white-space: nowrap; overflow: hidden; text-overflow: ellipsis;
  color: var(--text-primary);
}
.git-commit-time { flex-shrink: 0; font-size: var(--text-xs); color: var(--text-muted); }
```

**Pré-requisito de backend** — o `--pretty=format:"%h  %s  (%an, %ar)"` atual concatena tudo numa string; pra popular esses campos separadamente sem regex frágil, trocar por um formato delimitado e parsear em `git_ops.py`:

```python
# git_ops.py — log estruturado (separadores de controle que nunca aparecem em texto normal)
"log": ["log", "-n", "30", "--pretty=format:%h%x1f%an%x1f%ar%x1f%D%x1f%s%x1e"],
```
```python
def parse_log(raw: str) -> list[dict]:
    out = []
    for rec in raw.strip("\x1e").split("\x1e"):
        h, an, ar, refs, s = rec.split("\x1f")
        out.append({"hash": h, "author": an, "relDate": ar,
                     "refs": [r.strip() for r in refs.split(",") if r.strip()], "subject": s})
    return out
```

## 2. Grafo de commits compacto — o que dá pra fazer em ~350px

Grafo multi-lane de verdade (GitLens/TortoiseGit/Sourcetree) **não cabe** numa sheet de celular: cada branch/merge em paralelo consome uma coluna própria de ~14-16px, e esses clientes rodam em desktop com centenas de px sobrando. Confirmado nas fontes: GitLens tem um modo "Compact Graph Column Layout" justamente pra economizar espaço horizontal ("right-click the Graph column header and select Compact Graph Column Layout to reduce visual complexity", [GitLens](https://help.gitkraken.com/gitlens/gl-commit-graph/)), e mesmo assim ele reduz — não elimina — a necessidade de largura. TortoiseGit e Sourcetree têm o mesmo trade-off: coluna de grafo + coluna de mensagem lado a lado, pensado pra tela larga ([TortoiseGit Log Dialog](https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html)).

Os clientes mobile citados (Working Copy, GitHub mobile) resolvem isso **não tentando mostrar o grafo na lista principal**: a lista é linear/cronológica (uma linha por commit, sem lanes), e o grafo em árvore vira uma **visualização separada e opcional** — "commit history can also be presented in a tree graph which can be accessed from a repo's summary view" ([Working Copy manual](https://workingcopy.app/manual/repository-sheet/)); o GitHub mobile nem oferece grafo, só lista reverse-chronological com hash/autor/data ([GitHub community discussion #153686](https://github.com/orgs/community/discussions/153686)).

**Recomendação pro sheet estreito — degradar, não replicar o grafo:**

1. **Rail linear fino** (12-16px) à esquerda do dot/avatar, com um traço vertical contínuo ligando os commits — dá a sensação de "linha do tempo" sem tentar desenhar lanes paralelas. Renderiza com CSS puro (`border-left` no container + dot posicionado), sem canvas/SVG:
   ```css
   .git-commits { position: relative; padding-left: 2px; }
   .git-commits::before {
     content: ''; position: absolute; left: 11px; top: 20px; bottom: 20px;
     width: 1px; background: var(--border-default);
   }
   ```
2. **Merge commit** (2+ pais) vira um **glifo inline** na própria linha em vez de lane extra — ex. um ícone pequeno `⑂` ou badge "merge" antes do subject, com o subject virando "Merge branch 'X' into Y". É simplificação deliberada: perde a topologia visual exata de qual lane fez merge em qual, mas isso não é informação que se lê de forma útil em 350px de qualquer forma.
3. **Cor por branch**: se quiser reter *alguma* codificação de branch sem lanes, usar uma faixa colorida de 2-3px na borda esquerda do dot (mesma ideia do "Visual History" novo do GitLens 18, que despriorizou lanes por um "time-bucketed view of activity" com avatares — ver [GitLens Commit Graph](https://help.gitkraken.com/gitlens/gl-commit-graph/)) — mas isso é enhancement, não essencial pro MVP.
4. Grafo completo (multi-lane de verdade) fica reservado pra uma tela dedicada em paisagem/desktop (`min-width: 820px` — o breakpoint que o projeto já usa) — não pro sheet mobile.

> ponytail: rail linear + glifo de merge cobre 95% da leitura útil num sheet estreito; grafo multi-lane completo só quando o painel reancorar como sidebar desktop (`@media (min-width: 820px)`), não antes.

## 3. Fontes citadas

- [git-log — Git documentation](https://git-scm.com/docs/git-log) — placeholders `%h %s %an %ar %D`, convenções de `--oneline`
- [explainshell: git log --oneline --graph --decorate --all](https://explainshell.com/explain?cmd=git+log+--oneline+--graph+--decorate+--all) — semântica de cor do `--decorate` (verde local / vermelho remoto / azul HEAD / rosa stash)
- [Advanced Git Log — Atlassian Git Tutorial](https://www.atlassian.com/git/tutorials/git-log) — combinações usuais de flags de log
- [GitLens Commit Graph — GitKraken Help](https://help.gitkraken.com/gitlens/gl-commit-graph/) — colunas redimensionáveis, degradação texto→avatar em coluna estreita, "Compact Graph Column Layout"
- [TortoiseGit — Log Dialog](https://tortoisegit.org/docs/tortoisegit/tgit-dug-showlog.html) — estrutura em 3 painéis (grafo+lista / mensagem completa / arquivos alterados)
- [Working Copy — Repository actions (manual)](https://workingcopy.app/manual/repository-sheet/) — lista de commits em cliente iOS (hash, autor, data, mensagem), grafo em árvore como view separada, long-press pra ações
- [GitHub mobile: commit history discussion #153686](https://github.com/orgs/community/discussions/153686) — GitHub mobile usa lista cronológica simples, sem grafo
- [github/relative-time-element](https://github.com/github/relative-time-element) — componente oficial de timestamp relativo compacto/auto-updating usado no GitHub
- [Tower Git Client — Release Notes](https://www.git-tower.com/release-notes) — fallback de avatar pra iniciais quando não há imagem, agrupamento por data

**Arquivos relevantes no repo**: `/home/jefferson/pessoal/claude-pocket/frontend/src/components/GitSheet.svelte` (lista atual, padrão push-view do diff a reaproveitar), `/home/jefferson/pessoal/claude-pocket/backend/app/git_ops.py` (linha 91, formato do `git log` a estruturar).

## R2 — Syntax highlighting (Shiki vs hljs vs Prism)

## Comparativo: bibliotecas de syntax highlighting (viewer-only, self-contained, Svelte 5 + Vite)

Contexto do código atual: `frontend/src/components/GitSheet.svelte` já tem um diff viewer que faz parse de unified diff linha a linha (`line.startsWith('+')`, etc.) e renderiza num `<pre>` com `<span>` coloridos por classe CSS (`.add`/`.del`/`.hunk`/`.meta`), e um "log" que cai cru num `<pre>`. `frontend/package.json` não tem nenhuma lib de highlight ainda (só `lottie-web`, `qr-scanner`). Nota: o MCP do Context7 não estava disponível nesta sessão (não apareceu na lista de tools carregáveis via `ToolSearch`), então toda a pesquisa abaixo veio de `WebSearch`/`WebFetch` direto nas fontes oficiais.

### 1. Tabela comparativa

| Critério | **Shiki** | highlight.js | Prism | (referência) CodeMirror/Lezer | (referência) starry-night |
|---|---|---|---|---|---|
| Tamanho & tree-shaking | Fine-grained bundle (`shiki/core` + `@shikijs/langs/*` + `@shikijs/themes/*`) importa só o que você usa; código-split automático no Vite. Full bundle é 6.4MB/1.2MB gzip, mas ninguém usa isso em produção — [shiki.style/guide/bundles](https://shiki.style/guide/bundles) | Core + `registerLanguage()` por linguagem = pequeno; default (import cru) carrega ~192 linguagens — [highlightjs.readthedocs.io](https://highlightjs.readthedocs.io/en/latest/readme.html) | Menor "out of the box": core ~20KB, cada linguagem +3-30KB, arquivos por-linguagem tree-shakeable nativamente — [pkgpulse.com](https://www.pkgpulse.com/guides/shiki-vs-prismjs-vs-highlightjs-syntax-highlighting-2026) | Pesado: engine de editor completo (view+state+lang packages), overkill pra só exibir | Core+WASM 185KB; +250KB (~35 langs comuns) ou +1.6MB (~600 langs) — [github.com/wooorm/starry-night](https://github.com/wooorm/starry-night) |
| Grammars/temas 100% locais (sem CDN) | Sim — tudo ESM puro, nada de fetch remoto, herdeiro do Shikiji ("no more CDN, no more assets") — [shiki.matsu.io](https://shiki.matsu.io/guide/bundles) | Sim, sempre foi local | Sim, sempre foi local | Sim (pacotes `@lezer/*`) | Sim, mas WASM do Oniguruma precisa ser servido localmente (dá, mas é passo a mais) — [github.com/wooorm/starry-night/issues/8](https://github.com/wooorm/starry-night/issues/8) |
| Integração Vite/ESM | Primeira classe: subpath exports (`shiki/core`, `shiki/engine/javascript`), dynamic import por linguagem = chunk automático | Boa, mas import "tudo" por padrão se não usar `lib/core` | Boa, arquivos por linguagem já isolados | Boa mas complexa (muitos pacotes `@codemirror/*`) | Boa, ESM puro, mas precisa resolver a URL do `.wasm` no bundler |
| Sync vs async | `createHighlighter`/`createHighlighterCore` é **async na criação** (carrega engine/grammars); depois disso, **`codeToHtml`/`codeToTokens` são síncronos** — singleton de longa duração, chamado sync no render — [shiki.style/guide/install](https://shiki.style/guide/install) | 100% síncrono sempre | 100% síncrono sempre | Parsing incremental (Lezer), API própria, não é um simples sync/async de "highlight this string" | Criação da instância é async (carrega WASM); highlight em si é sync depois |
| Qualidade do tema (parecido com VSCode) | **Máxima** — usa grammars TextMate reais + temas literalmente extraídos do VS Code (`dark-plus`/`light-plus`, `github-dark/light`, `vitesse-*`) | Boa mas CSS-classe genérica, tokenização menos granular que TextMate (JSX/TS às vezes perde nuance) | Boa, mas temas são recriações community, não os arquivos reais do VSCode | Depende do theme package escolhido (`@codemirror/theme-one-dark` etc.), não usa grammar TextMate | Igual ao GitHub.com (que é o padrão-ouro visual, mas não é literalmente VSCode) |
| Highlight de diff | Sem suporte "out of the box" pra unified diff cru, mas o pacote oficial `@shikijs/transformers` tem `transformerNotationDiff` pro padrão `[!code ++]/[!code --]`; pra diff real (`git diff`) o recipe é: tokenizar o código sem os prefixos +/- e recolorir por linha (mesma técnica do plugin `diff-highlight` do Prism) — [shiki.style/packages/transformers](https://shiki.style/packages/transformers), [github.com/shikijs/shiki/issues/1031](https://github.com/shikijs/shiki/issues/1031) | Sem suporte nativo a diff; mesmo recipe manual | Tem plugin oficial `prism-diff-highlight` pronto, mas menos flexível de customizar | Não é o foco da lib | Sem suporte nativo a diff |
| DX no Svelte | `{@html highlighter.codeToHtml(...)}` ou iterar `codeToTokens` pra controlar o markup (necessário aqui, pra manter a estrutura de linha que o GitSheet já tem) | `{@html hljs.highlight(...).value}` — simples | `{@html Prism.highlight(...)}` — simples | Precisa montar um `EditorView` — muito mais código que "exibir string colorida" | `{@html toHtml(tree)}` (hast→html) — simples |
| Mobile / regex engine | Engine JS nativo (`createJavaScriptRegexEngine`, via `oniguruma-to-es`) evita o WASM do Oniguruma (~1.5MB) — usa **~4% do tamanho**, recomendado explicitamente pra client-side/mobile — [shiki.style/guide/regex-engines](https://shiki.style/guide/regex-engines) | Sem WASM, sempre leve | Sem WASM, sempre leve | Sem WASM (Lezer é JS puro) | **Depende de WASM Oniguruma** sempre — pior fit pra mobile |

### 2. Recomendação

**Shiki**, no modo *fine-grained bundle* + **engine JS** (`createJavaScriptRegexEngine`, sem WASM), com import dinâmico por linguagem.

Justificativa pro conjunto de constraints deste projeto:
- O requisito "parecido com VSCode" é literal: os temas `dark-plus`/`light-plus` do Shiki **são** os temas reais do VS Code portados, e a tokenização usa as mesmas grammars TextMate — nenhuma outra lib bate isso em fidelidade.
- O medo de peso de bundle/WASM (relevante pra rodar em LAN/Tailscale, no celular) é resolvido pelo engine JS do Shiki: descarta o WASM do Oniguruma (~1.5MB) e some pra ~4% do tamanho, rodando regex nativo. Isso neutraliza a maior crítica histórica ("Shiki é pesado") — ela vale pro *bundle completo*, não pro fine-grained + JS engine.
- self-contained/offline: 100% ESM, sem CDN — mesmo critério que highlight.js/Prism cumprem, mas Shiki cumpre com qualidade de tema muito superior.
- starry-night tem qualidade parecida (GitHub-style) mas obriga WASM sempre — pior pra mobile, sem ganho real sobre Shiki+JS-engine.
- CodeMirror/Lezer é overkill: é motor de editor (view+state), não um "syntax highlighter de string" — não serve pro caso viewer-only, e a qualidade do tema depende de pacotes extra que não batem com TextMate/VSCode.
- highlight.js e Prism são mais simples e 100% síncronos, mas perdem no critério mais pesado do pedido (qualidade visual "estilo VSCode") e não têm um caminho oficial pra diff tão bom quanto o Prism plugin — que também não bate a fidelidade de tema do Shiki.

Trade-off aceito: criação do highlighter é assíncrona (uma vez, no boot ou lazy no primeiro diff aberto) — não é problema aqui porque o `GitSheet` já é async (`diffLoading` já existe no fluxo atual).

### 3. Esboço de integração Vite/Svelte (self-contained, só as linguagens usadas)

```bash
npm --prefix frontend install shiki
```

`frontend/src/lib/highlight.ts` — singleton, engine JS (sem WASM), só os temas/linguagens que o projeto realmente usa (stack do repo: TS/JS/Svelte, Python, Bash, JSON/YAML/Markdown, CSS/HTML):

```ts
import { createHighlighterCore, type HighlighterCore } from 'shiki/core';
import { createJavaScriptRegexEngine } from 'shiki/engine/javascript';

// temas reais do VS Code — casam com o toggle dark/light do app (data-theme)
import darkPlus from '@shikijs/themes/dark-plus';
import lightPlus from '@shikijs/themes/light-plus';

// ponytail: mapa fixo das linguagens do próprio repo; linguagem nova = 1 import a mais.
const LANG_LOADERS: Record<string, () => Promise<any>> = {
  ts: () => import('@shikijs/langs/typescript'),
  tsx: () => import('@shikijs/langs/tsx'),
  js: () => import('@shikijs/langs/javascript'),
  svelte: () => import('@shikijs/langs/svelte'),
  py: () => import('@shikijs/langs/python'),
  sh: () => import('@shikijs/langs/bash'),
  json: () => import('@shikijs/langs/json'),
  yaml: () => import('@shikijs/langs/yaml'),
  md: () => import('@shikijs/langs/markdown'),
  css: () => import('@shikijs/langs/css'),
  html: () => import('@shikijs/langs/html'),
};

function langFromPath(path: string): string {
  const ext = path.split('.').pop() ?? '';
  return ext in LANG_LOADERS ? ext : 'txt';   // sem grammar -> plain text (fallback, não quebra)
}

let core: HighlighterCore | null = null;
const loadedLangs = new Set<string>();

async function getCore(): Promise<HighlighterCore> {
  if (!core) {
    core = await createHighlighterCore({
      themes: [darkPlus, lightPlus],
      langs: [],
      engine: createJavaScriptRegexEngine(),   // sem WASM — leve, ok pra mobile
    });
  }
  return core;
}

export async function ensureLang(lang: string) {
  if (lang === 'txt' || loadedLangs.has(lang)) return;
  const loader = LANG_LOADERS[lang];
  if (!loader) return;
  await core!.loadLanguage(await loader());
  loadedLangs.add(lang);
}

export { getCore, langFromPath };
```

Uso num componente: chamar `getCore()` uma vez (top-level, ou no primeiro `openDiff`), depois `ensureLang(lang)` sob demanda por arquivo — só baixa o grammar da linguagem realmente aberta.

### 4. Aplicando num DIFF de git (adaptação direta do `GitSheet.svelte` atual)

O ponto chave: **não dá pra tokenizar linha-a-linha isolada** — grammars TextMate são stateful entre linhas (string/comentário multi-linha quebra se cada linha for highlighted sozinha). Recipe correto: separar as linhas de código reais das linhas de meta/hunk, remontar o "código puro" (sem o prefixo `+`/`-`/espaço) em um blob contínuo por hunk, tokenizar esse blob inteiro de uma vez, e depois recasar os tokens de volta linha a linha com a classe de diff (`add`/`del`) que o app já usa.

```ts
// dentro de openDiff(), depois de obter diffText:
const lang = langFromPath(path);
await ensureLang(lang);
const hl = await getCore();

const rawLines = diffText.split('\n');
type Row = { kind: 'add' | 'del' | 'ctx' | 'meta' | 'hunk'; code: string };
const rows: Row[] = rawLines.map((l) => {
  if (l.startsWith('@@')) return { kind: 'hunk', code: l };
  if (l.startsWith('diff ') || l.startsWith('index ') || l.startsWith('+++') || l.startsWith('---'))
    return { kind: 'meta', code: l };
  if (l.startsWith('+')) return { kind: 'add', code: l.slice(1) };
  if (l.startsWith('-')) return { kind: 'del', code: l.slice(1) };
  return { kind: 'ctx', code: l.startsWith(' ') ? l.slice(1) : l };
});

// tokeniza só as linhas de código, como um único blob (preserva estado da grammar)
const codeRows = rows.filter((r) => r.kind !== 'meta' && r.kind !== 'hunk');
const tokenLines = hl.codeToTokensBase(codeRows.map((r) => r.code).join('\n'), {
  lang, theme: isLightTheme() ? 'light-plus' : 'dark-plus',   // isLightTheme() lê o data-theme atual
}).tokens;   // array de linhas, cada uma array de tokens {content, color}

// recasa: zip tokenLines de volta em codeRows, na ordem
codeRows.forEach((r, i) => (r.tokens = tokenLines[i]));
```

No template, cada linha de código passa a renderizar seus `tokens` (`<span style="color:{t.color}">{t.content}</span>`) **dentro** do mesmo `<span class:add class:del>` que já existe hoje — troca só o conteúdo interno, mantém 100% do CSS de fundo add/del/hunk/meta já escrito no `GitSheet.svelte` (`.git-diff .add`, `.git-diff .del`, etc.). Linhas `hunk`/`meta` continuam como texto puro, sem highlight.

Isso também serve pro botão "log" (que hoje cai cru num `<pre>` feio): se quiser, dá pra rodar o mesmo `codeToTokensBase` com `lang: 'diff'` — só que `diff` como linguagem de grammar (cores fixas por +/-/hash de commit) é mais simples ainda, sem precisar detectar linguagem de arquivo nenhuma.

### 5. Fontes citadas

- [shiki.style/guide/bundles](https://shiki.style/guide/bundles) — full bundle (6.4MB/1.2MB gzip), web bundle (3.8MB/695KB gzip), fine-grained bundle via `shiki/core`.
- [shiki.matsu.io/guide/bundles](https://shiki.matsu.io/guide/bundles) — histórico Shikiji, "no more CDN, no more assets".
- [shiki.style/guide/install](https://shiki.style/guide/install) — `createHighlighter` async, `codeToHtml`/`codeToTokens` síncronos após criação, singleton recomendado.
- [shiki.style/guide/regex-engines](https://shiki.style/guide/regex-engines) — engine JS (`oniguruma-to-es`) vs Oniguruma WASM: ~4% do tamanho, recomendado pra client-side/mobile.
- [shiki.style/packages/transformers](https://shiki.style/packages/transformers) — `transformerNotationDiff`, classes `diff`/`add`/`remove`.
- [github.com/shikijs/shiki/issues/1031](https://github.com/shikijs/shiki/issues/1031) — proposta de diff highlighting nativo, recipe "tokenize + recolor by prefix" (mesma técnica do `prism-diff-highlight`).
- [pkgpulse.com/guides/shiki-vs-prismjs-vs-highlightjs-syntax-highlighting-2026](https://www.pkgpulse.com/guides/shiki-vs-prismjs-vs-highlightjs-syntax-highlighting-2026) — tamanhos Prism (~20KB core + 3-30KB/lang) vs Shiki full (~250KB-1MB de grammar payload).
- [highlightjs.readthedocs.io/en/latest/readme.html](https://highlightjs.readthedocs.io/en/latest/readme.html) — 192 linguagens no default import; `lib/core` + `registerLanguage` pra subset.
- [github.com/wooorm/starry-night](https://github.com/wooorm/starry-night) e [readme.md](https://github.com/wooorm/starry-night/blob/main/readme.md) — bundle 185KB (core+WASM), +250KB (~35 langs) / +1.6MB (~600 langs), depende de WASM Oniguruma sempre.
- [github.com/wooorm/starry-night/issues/8](https://github.com/wooorm/starry-night/issues/8) — suporte a WASM hospedado localmente (não-CDN).

Arquivos do projeto lidos/relevantes: `/home/jefferson/pessoal/claude-pocket/frontend/src/components/GitSheet.svelte`, `/home/jefferson/pessoal/claude-pocket/frontend/package.json`, `/home/jefferson/pessoal/claude-pocket/frontend/src/app.css` (mecanismo `data-theme="light"` no `:root`, usado acima pra escolher `dark-plus`/`light-plus`).

## R3 — Commit graph (algoritmo de lanes)

# Pesquisa: Commit Graph (lanes/branches) num sheet estreito mobile

## 1) Abordagem recomendada: SVG feito à mão (layout calculado no backend), não uma lib

**Motivo de descartar `@gitgraph/js` / `@gitgraph/react`:** o gitgraph.js está **arquivado desde jul/2024** ("unmaintained since 2019"), e mais grave — não é feito pra isso. É uma **API imperativa** (`gitgraph.branch(...).commit(...).merge(...)`) pensada pra ilustrar posts de blog/apresentações, não pra importar um `git log --parents` real. O próprio autor recomenda hoje olhar Mermaid.js em vez disso ([github.com/nicoespeon/gitgraph.js](https://github.com/nicoespeon/gitgraph.js/), [npmjs.com/package/@gitgraph/js](https://www.npmjs.com/package/@gitgraph/js)).

**`dolthub/commit-graph`** é a referência mais próxima do que você quer — SVG, aceita `{sha, parents[], commit:{author,message}}` (formato estilo GitHub API), já implementa o algoritmo de branch/merge children, e tem `CommitGraph.WithInfiniteScroll` pra paginação ([github.com/dolthub/commit-graph](https://github.com/dolthub/commit-graph), [dolthub.com/blog/.../drawing-a-commit-graph](https://www.dolthub.com/blog/2024-08-07-drawing-a-commit-graph/)). Só que é **componente React**, não existe porte Svelte mantido (busquei, não achei nada sério).

**Conclusão prática pro claude-pocket:** não vale puxar dependência nenhuma pra isso.
- O algoritmo de lanes é ~30-50 linhas (seção 3) e dá pra rodar **no backend Python**, que já tem toda a lógica git em `git_ops.py`.
- O frontend Svelte só recebe `{col, parentEdges}` já prontos e desenha um `<svg>` trivial — zero JS de grafo no client, zero bundle extra, 100% self-contained (constraint do projeto).
- SVG > Canvas aqui: a escala é ~30-50 commits por página (não milhares), então o ganho de canvas citado nos benchmarks do pvigier só aparece na *virtualização de viewport* (0.58ms vs 106ms renderizando tudo), não na escolha SVG-vs-canvas em si ([pvigier.github.io](https://pvigier.github.io/2019/05/06/commit-graph-drawing-algorithms.html)). SVG dá hit-testing por commit de graça (tap no dot → abre diff), fica nítido em qualquer DPI de tela de celular, e não precisa de loop de redraw ao trocar tema light/dark.
- Pro sheet estreito: cap de **~4-5 lanes visíveis** (dot+linha ~14-16px cada, gutter total ~60-80px) e o gutter do grafo rola em `overflow-x` **independente** da coluna de texto (hash/msg/autor) — mesma convenção que o projeto já usa pra diff/log largo. Se o histórico tiver mais lanes que isso, colapsar merges como o Fork faz (clique expande/colapsa o merge) em vez de espremer o grafo ([fork.dev/blog/posts/collapsible-graph](https://fork.dev/blog/posts/collapsible-graph/)); GitLens usa a mesma filosofia de "esconder detalhe quando a coluna aperta" pras colunas de autor ([help.gitkraken.com/gitlens/gl-commit-graph](https://help.gitkraken.com/gitlens/gl-commit-graph/)).

## 2) Formato de `git log` que o backend deve emitir

Hoje `git_ops.py` só tem `"log": ["log", "-n", "30", "--pretty=format:%h  %s  (%an, %ar)"]` (uma linha, sem parents/refs — não dá pra montar grafo). Trocar por um formato delimitado por bytes de controle (truque padrão, evita quebrar em `\n`/`|` dentro da mensagem — usado pelo TIL do Simon Willison e pelo `jc`) ([til.simonwillison.net/jq/git-log-json](https://til.simonwillison.net/jq/git-log-json)):

```
git -C <cwd> log --topo-order -n 50 --skip=<offset> \
  --pretty=format:%H%x1f%P%x1f%D%x1f%an%x1f%ct%x1f%s%x1e
```

Campos (documentados em [git-scm.com/docs/pretty-formats](https://git-scm.com/docs/pretty-formats)):
- **`%H`** — hash completo (não `%h`; precisa bater char-a-char com `%P` pra montar as arestas).
- **`%P`** — hashes dos parents, completos, separados por espaço. O 1º é o "branch parent" (continua a lane), os demais são "merge parents".
- **`%D`** — refs decoradas (branch/tag/HEAD), **sem parênteses** (melhor que `%d` pra parsear).
- **`%an`**, **`%ct`** (timestamp unix do committer — chave de ordenação estável, não string de data), **`%s`** (assunto).
- Separador de campo `%x1f` (Unit Separator) e de registro `%x1e` (Record Separator) — split limpo em Python (`stdout.split('\x1e')`, cada linha `.split('\x1f')`).
- **`--topo-order`**: evita intercalar commits de branches paralelas por data, que fica confuso num grafo — é a flag que o próprio doc do git recomenda pra uso com `--graph` ([git-scm.com/docs/git-log](https://git-scm.com/docs/git-log)).
- Paginação via `-n`/`--skip` (igual ao padrão de infinite-scroll do dolthub) em vez de carregar o histórico inteiro — o projeto já janela a lista de mensagens em 120 itens (`MessageList.svelte`), aplicar o mesmo princípio aqui.
- Opcional: se `list_branches().dirty` for true, injetar uma linha sintética "uncommitted changes" no topo do grafo (padrão comum em GUIs de git) usando o dado que `git_ops.py` já calcula.

## 3) Esboço da lógica de atribuição de lanes

Baseado no algoritmo de "straight branches" descrito pelo pvigier (usado por GitKraken/gitk-like tools) e simplificado pelo dolthub (3 casos por tipo de filho) ([pvigier.github.io](https://pvigier.github.io/2019/05/06/commit-graph-drawing-algorithms.html), [dolthub.com blog](https://www.dolthub.com/blog/2024-08-07-drawing-a-commit-graph/)):

```
lanes: list[str|None] = []   # índice = coluna; valor = hash do commit "esperado" nessa lane

for commit in commits_em_ordem_topologica:   # do log, já vem child antes de parent
    # 1. onde ele já é esperado?
    col = lanes.index(commit.hash) if commit.hash in lanes else None
    if col is None:
        col = next_free_slot(lanes) or append_new(lanes)   # commit "raiz" de uma lane nova (branch head)

    node[commit.hash] = col

    # 2. libera/realoca a lane pros parents
    if not commit.parents:
        lanes[col] = None                        # commit inicial -> fecha a lane
    else:
        first, *merge_parents = commit.parents
        lanes[col] = first                        # 1º parent = "branch parent", continua reto na mesma coluna

        for p in merge_parents:                   # parents extras = merges
            if p in lanes:
                edge(commit, p, curva_para=lanes.index(p))   # já tem lane -> curva até ela, sem alocar
            else:
                pcol = next_free_slot(lanes) or append_new(lanes)
                lanes[pcol] = p
                edge(commit, p, curva_para=pcol)
```

Pontos-chave:
- **Reuso de slot livre (`None`)** em vez de sempre `append` evita que o grafo cresça sem necessidade quando uma branch termina e outra nasce depois — é a otimização central citada tanto pelo pvigier quanto pelo dolthub.
- **1º parent = branch child** → aresta reta, mesma coluna, mesma cor (é isso que faz o grafo parecer "trilhos" no gitk). Parents extras (merge) → aresta curva (bezier simples: `M x0,y0 C x0,ym x1,ym x1,y1`) que pode nascer numa coluna nova à direita.
- Isso roda **uma vez no backend** por página de 50 commits — output final pro frontend é só `[{hash, col, parents: [{hash, col, curved: bool}], ...msg/author/date}]`. O Svelte desenha um `<svg>` com dots+paths a partir disso, sem reimplementar o algoritmo no client.
- Testável com um `assert`/`demo()` simples (linear + 1 merge + 1 branch nova) cobre os 3 casos acima — não precisa de suíte grande pra isso.

## 4) Fontes citadas

- [pvigier's blog — Commit Graph Drawing Algorithms](https://pvigier.github.io/2019/05/06/commit-graph-drawing-algorithms.html) — algoritmo de lanes "straight branches", comparação gitk/GitExtensions/GitKraken/SourceTree, benchmarks canvas/SVG/virtualização.
- [DoltHub Blog — Drawing a commit graph](https://www.dolthub.com/blog/2024-08-07-drawing-a-commit-graph/) — implementação prática (branch vs merge children), estrutura de dados de colunas.
- [github.com/dolthub/commit-graph](https://github.com/dolthub/commit-graph) — componente React SVG de referência, formato de input, infinite scroll.
- [github.com/nicoespeon/gitgraph.js](https://github.com/nicoespeon/gitgraph.js/) e [npmjs.com/package/@gitgraph/js](https://www.npmjs.com/package/@gitgraph/js) — status arquivado, API imperativa (não consome `git log` real).
- [git-scm.com/docs/git-log](https://git-scm.com/docs/git-log) — `--parents`, `--topo-order`.
- [git-scm.com/docs/pretty-formats](https://git-scm.com/docs/pretty-formats) — `%H %P %D %an %ct %s`, `%x<hex>`.
- [til.simonwillison.net/jq/git-log-json](https://til.simonwillison.net/jq/git-log-json) — truque de delimitador `\x1f`/`\x1e` pra sair de `git log --pretty` sem quebrar em mensagens com newline/pipe.
- [help.gitkraken.com/gitlens/gl-commit-graph](https://help.gitkraken.com/gitlens/gl-commit-graph/) — colunas encolhem/viram ícone em tela estreita.
- [fork.dev/blog/posts/collapsible-graph](https://fork.dev/blog/posts/collapsible-graph/) — colapsar merges como estratégia de largura.
- [github.com/mhutchie/vscode-git-graph](https://github.com/mhutchie/vscode-git-graph) — extensão popular, confirma que não há wrapper "puxa git log e desenha" pronto e mantido fora do que cada tool implementa por si.

## R4 — Sheet redimensionável + diff/paths UX

## 1. Painel redimensionável (drag-handle) — desktop

- **Mecânica de arrasto**: `pointerdown` no handle → `element.setPointerCapture(event.pointerId)` prende os eventos de ponteiro nesse elemento mesmo se o cursor sair da área visual do handle durante o arrasto → `pointermove` recalcula a largura → `pointerup` chama `releasePointerCapture()`. É o padrão canônico (MDN).
- **Referência de lib madura**: `react-resizable-panels` (bvaughn) — `Panel`/`PanelGroup`/handle de resize com `minSize`/`maxSize` em %; usa **capture phase** em `pointerdown`/`pointerup` (compatibilidade com libs de UI que fazem stopPropagation); tem `hitAreaMargins` — área de toque maior que a linha visual do handle (default 15px touch / 5px mouse), relevante pra pegar o handle fácil sem engordar visualmente a borda.
- **Persistência**: salvar no `localStorage` só no `pointerup`/fim do drag (callback tipo `onLayoutChanged`), não a cada `pointermove` — evita spam de writes. É exatamente o padrão que o resize-handle do `Sidebar` do próprio app já usa; dá pra reaproveisar o mesmo hook/lógica pro handle do `GitSheet`.
- **Mobile (sheet, não painel lateral)**: em vez de resize horizontal, o padrão é resize vertical por **detents** (alturas fixas) com um **grabber** — barra horizontal no topo, arrastável e também "tap pra ciclar" entre alturas (Apple HIG "Sheets"; Material Design 3 "Bottom sheets" faz a área dos 48dp do topo inteira interativa, não só a barrinha visual — touch target maior que o traço). NN/g alerta pra não depender só do gesto de arrastar/fechar em mobile: gesto pode colidir com gestos do sistema (notification drawer, control center) — manter um botão de fechar explícito como fallback.

## 2. Caminho de arquivo longo compacto

- **Regra consolidada** (Windows `PathCompactPathEx`, VS Code breadcrumbs, editores em geral): truncar **no meio**, nunca no fim — preserva início (raiz/primeiras pastas) e o fim (nome do arquivo + extensão), que são as partes que o usuário usa pra reconhecer o caminho. Cortar a extensão ou o basename é o erro mais comum.
- **Técnica com medição real** (quando precisa ser pixel-perfect): medir a largura do texto num elemento invisível, escutar `ResizeObserver` no container pra recalcular ao redimensionar, `startLength = Math.ceil((maxLength - ellipsisWidth) / 2)`, resto vira o `endLength`.
- **Atalho CSS-only** (mais barato, sem JS de medição, self-contained): container flex — `div` do diretório com `flex-shrink:1; overflow:hidden; text-overflow:ellipsis; direction:rtl` (o `rtl` faz a reticência aparecer no início da string, "escondendo" o meio/começo do path e mantendo o fim visível) + `span` do basename com `flex-shrink:0` fixo no fim. Resolve sem canvas/ResizeObserver.
- **Estilo visual**: basename com peso/cor de destaque, diretório com opacidade/tamanho reduzido — é o padrão do breadcrumb do VS Code (segmentos de path esmaecidos exceto o atual) e de terminais modernos.

## 3. Diff viewer mobile

- **Unified é o default certo pra mobile**, split (side-by-side) só quando tem largura (>820px, que já é o breakpoint do app). GitLab tem queixa documentada de review mobile ruim justamente porque força colunas lado a lado em tela estreita, gerando wrap pesado e ilegível. GitHub reverteu isso: hoje até o unified faz wrap automático de linha longa (antes só o split tinha).
- **Linha longa**: quebrar (wrap), não scroll horizontal, em mobile — bibliotecas de referência (`git-diff-view`) expõem isso como prop booleana (`diffViewWrap`), CSS puro (`white-space: pre-wrap` + indent pra não confundir linha quebrada com linha nova).
- **Virtualização pra diffs grandes**: renderizar só as linhas visíveis + overscan pequeno. `react-window` é a lib de referência do ecossistema (`FixedSizeList`/`VariableSizeList`, prop `overscanCount`, default 1) — como diff é monoespaçado, `FixedSizeList` (altura de linha fixa) já basta, não precisa da variante de tamanho variável. Em mobile, manter overscan baixo (1–3 linhas): mais overscan = scroll mais suave mas mais DOM/CPU, o que penaliza mais em device fraco. Libs de diff que já fazem isso: `react-virtualized-diff` (10k+ linhas), `react-diff-viewer-continued` (virtualiza linhas fora da viewport), Pierre `CodeView`/diffs.com (estima a altura de cada linha matematicamente, corrige e cacheia a estimativa enquanto rola, em vez de medir tudo de uma vez).
- **Prático pro claude-pocket**: como o app é self-contained/sem CDN, não precisa puxar `react-window` inteiro — dá pra fazer virtualização caseira simples (slice do array de linhas visível calculado por `scrollTop / lineHeight` fixa, já que é monoespaçado) reaproveitando a mesma ideia de overscan pequeno. Unified como modo único no mobile (full-width sheet); split só quando o `GitSheet` estiver docado como painel lateral desktop com espaço de verdade.

## 4. Fontes citadas

- [Element: setPointerCapture() — MDN](https://developer.mozilla.org/en-US/docs/Web/API/Element/setPointerCapture)
- [Pointer events — MDN](https://developer.mozilla.org/en-US/docs/Web/API/Pointer_events)
- [react-resizable-panels — GitHub (bvaughn)](https://github.com/bvaughn/react-resizable-panels)
- [Sheets — Apple Human Interface Guidelines](https://developer.apple.com/design/human-interface-guidelines/sheets)
- [Bottom sheets – Material Design 3 (accessibility)](https://m3.material.io/components/bottom-sheets/accessibility)
- [BottomSheetDragHandleView — Android Developers](https://developer.android.com/reference/com/google/android/material/bottomsheet/BottomSheetDragHandleView)
- [Bottom Sheets: Definition and UX Guidelines — NN/g](https://www.nngroup.com/articles/bottom-sheet/)
- [Twice the Challenge: Optimising Middle Truncation for Modern UIs — Medium (Anıl Pak)](https://medium.com/@anilpak35/middle-truncate-8134e9236068)
- [Shortening Long File Paths — Coding Horror](https://blog.codinghorror.com/shortening-long-file-paths/)
- [VS Code User Interface / Breadcrumbs docs](https://code.visualstudio.com/docs/getstarted/userinterface)
- [Wrap long lines in unified diff view — GitHub community discussion #8497](https://github.com/orgs/community/discussions/8497)
- [git-diff-view — GitHub (MrWangJustToDo)](https://github.com/MrWangJustToDo/git-diff-view)
- [git-diff-view — demo site](https://mrwangjusttodo.github.io/git-diff-view/)
- [react-diff-view — GitHub (otakustay)](https://github.com/otakustay/react-diff-view)
- [react-diff-viewer-continued — npm](https://www.npmjs.com/package/react-diff-viewer-continued)
- [react-virtualized-diff — GitHub](https://github.com/Zhang-JiahangH/react-virtualized-diff)
- [Diffs, from Pierre — CodeView docs](https://diffs.com/docs)
- [Virtualize large lists with react-window — web.dev](https://web.dev/articles/virtualize-long-lists-react-window)
- [Merge request diffs development guide — GitLab Docs](https://docs.gitlab.com/development/merge_request_concepts/diffs/)
- [Mobile web UI usability in merge request review — GitLab Forum](https://forum.gitlab.com/t/mobile-web-ui-usability-in-merge-request-review/59266)