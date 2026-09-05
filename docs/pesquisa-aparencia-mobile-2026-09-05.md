# Pesquisa: aparência do app nativo (paridade visual com a PWA e além)

Data: 05/09/2026. Atualização dirigida da pesquisa de 21/08 (`pesquisa-expo-2026-08-21.md`),
feita antes do plano de aparência. Três frentes em paralelo: o que a pilha visual mudou desde
21/08 · referências de apps nativos de agente de código · inventário do que a PWA faz na visão
celular e o que o nativo já tem. Complementa, não substitui, a pesquisa de 21/08.

## Ponto de partida (medido hoje no emulador)

Os dois no mesmo tema escuro, mesma sessão: os tokens de cor são os mesmos (`packages/core/src/theme.ts`
espelha o `app.css`), mas o nativo está numa etapa anterior da PWA. Os planos 1 (fundação) e 2
(paridade) entregaram funcionalidade; nenhum dos 71 commits da branch é de aparência. Da pilha visual
decidida em 21/08, **quatro libs nunca foram instaladas**: `@lodev09/react-native-true-sheet`,
`sonner-native`, `lucide-react-native`, `expo-symbols` (e `expo-sharing`, que a spec precisa, também
não). O que está instalado e em uso: `expo-glass-effect`, `expo-blur` (num `ui/Glass.tsx` que já
existe), `@react-native-menu/menu`, `react-native-unistyles`, `react-native-enriched-markdown`,
`@legendapp/list` (lista de sessões, chat e espelho do terminal), `expo-localization`.

## 1. Pilha visual: o que mudou desde 21/08

Versões (instalada → última em 05/09): expo 57.0.15→57.0.20 · reanimated 4.5.1→4.6.0 ·
keyboard-controller 1.21.9→1.22.4 · @expo/ui 57.0.12→57.0.16 · streamdown 0.2→0.3.0. Sem mudança:
glass-effect 57.0.1, blur 57.0.2, unistyles 3.3.0 (jul/2026, sem 3.4/4.x), enriched-markdown 1.0.2,
@react-native-menu/menu 2.0.0 (última publicação set/2025).

- **Nada regrediu nem foi descontinuado.** SDK 57.0.16–20 é "zero breaking changes"
  (expo.dev/changelog/sdk-57). Único breaking no período: `@expo/ui` 57.0.14 removeu a prop `style`
  de `RNHostView` no Android (CHANGELOG expo-ui). Reanimated 4.6.0 traz callbacks de transição CSS e
  `contrastColor()`. keyboard-controller 1.22.2 melhorou `KeyboardChatScrollView` e dismissal
  interativo no Android (relevante pro composer).
- **`experimentalBlurMethod` → `blurMethod`** (renomeado no expo-blur 55.0.0, jan/2026). A pesquisa de
  21/08 cita o nome velho.
- **Blur não atravessa `Modal` no Android** (expo/expo#44165, aberta): `BlurTargetView` não referencia
  conteúdo fora do Modal. Sheet/drawer que precisa de blur de fundo não pode ser um `Modal` do RN.
- **`expo-glass-effect`**: `setColorScheme()` não chama `updateEffect()` — o material não atualiza ao
  trocar tema (expo/expo#43743); artefatos ao combinar glass-effect + blur no iOS 26 (#42501) seguem
  abertos. `@callstack/liquid-glass` 0.8.1 é só iOS; não substitui a dupla, é alternativa pro efeito
  iOS 26. Material 3 Expressive (Android 16) trouxe blur ao sistema, não exposto a apps.
- **`@react-native-menu/menu` é o risco da pilha**: parado desde set/2025, com issues abertas de build
  em RN 0.87 (`RCTBridge.h` não encontrado, #1221, 14/08/2026) e preview em branco com Fabric
  (#1201). `zeego` 3 é casca sobre a mesma lib, herda o risco. Plano B mantido: `@expo/ui`
  `ContextMenu`/`Menu` (SwiftUI/Compose, dentro do SDK 57). Decisão: buildar cedo a tela de menu
  contra RN 0.86 real; falhou → `@expo/ui`, não `zeego`.
- **unistyles 3.3.0** segue coerente com RN 0.86 + Reanimated 4.6 (integração oficial
  `react-native-unistyles/reanimated`); nenhuma issue de incompatibilidade encontrada.
- **Ícones**: a Expo publicou em 09/06/2026 que `@expo/vector-icons` está em depreciação e recomenda
  pacotes por família, citando Lucide nominalmente (expo.dev/blog/moving-away-from-expo-vector-icons).
  `lucide-react-native` (já decidido, nunca instalado) ganha endosso direto. `expo-symbols` segue beta.
- **Superfície translúcida sobre papel de parede**: sem mudança de padrão. Blur real (`expo-blur`)
  custa mais no Android; overlay `rgba` sem blur pra card/chip em lista rolando. É a regra que o
  `CLAUDE.md` já registra pra PWA (`--surface-raised`/`--surface-inset` sobre `--glass-panel`).

## 2. Referências: apps nativos de agente de código (2025–2026)

- **Happy** (slopus/happy, base dos componentes de ferramenta do nosso app): AskUserQuestion virou view
  nativa com radio/checkbox; sem agrupamento de ferramentas nem pensamento documentados. Issue #59
  mostra erro JSON cru na conversa — risco que a PWA já cobre com `renderMarkdown`.
- **Claude app (Remote Control)**: aba "Code" dentro do app Claude; máquinas como cards, sessão nasce
  do toque; conversa em sync terminal ↔ web ↔ app (code.claude.com/docs/mobile).
- **ChatGPT app (Codex, mai/2026)**: threads, aprovar comando, ver diff, redirecionar tarefa, pareamento
  por QR. "Thinking" é escolha de **modelo**, não bloco de raciocínio na UI.
- **Omnara**: app dedicado (iOS/Android/Watch) — progresso, diff, aprovação, preview de localhost, voz,
  subagentes num painel. Concorrente direto em proposta; sem detalhe público de agrupamento.
- **Moshi** (terminal iOS pra agente de código) — a referência mais útil: (a) **sessões como cards com
  thumbnail ao vivo**, arrastar/minimizar/voltar com buffer intacto; (b) **Chat View**: terminal cru
  vira conversa com mensagens e **tool cards** separados, diff e web preview embutidos (getmoshi.app).
- **Listas densas com estado ao vivo**: Vercel redesenhou deployments pra mais densidade e agrupamento
  por estado/ambiente com branch inline (vercel.com/changelog/redesigned-deployments-list); Buildkite
  condensa histórico em barras (altura = duração, cor = status). Termius/Blink são o piso: lista
  morta, nome + estado binário.
- **Vidro (Apple HIG, Liquid Glass)**: "não use Liquid Glass na camada de conteúdo" — é camada
  funcional pra controle e navegação (tab bar, sidebar, sheet) que flutua sobre o conteúdo; conteúdo
  usa materiais padrão; respeitar Reduce Transparency e contraste em elemento crítico. É literalmente a
  regra "vidro no painel, superfície própria dentro dele" do `CLAUDE.md`. Material 3 Expressive
  confirma a mesma linguagem no Android (painel translúcido, cards com solidez própria).
- **Componentes (desde ago/2026)**: `true-sheet` 3.11 (sheet nativa, exige New Arch, teto de 3
  detents) vs `gorhom` 5 (JS, snap points ilimitados); `formSheet` do expo-router aceita N detents no
  iOS mas **Android trunca em 3** — projetar sempre com ≤3; regressão de jun/2026 no iOS 26
  (`nativeContainerStyle` vs conteúdo transparente, expo/expo#47831) ainda oscilando. Toast:
  `sonner-native` 0.27 (New Arch) ou `burnt` (nativo iOS/Android), ambos ativos.

**Conclusão da frente 2:** nenhum concorrente pesquisado agrupa ferramentas ou mostra pensamento melhor
que a PWA do Hangar. A barra de aparência é a própria PWA; a referência externa que acrescenta algo é
o Moshi (tool card curto por chamada, sessão como card com prévia ao vivo).

## 3. Inventário: PWA (celular) × nativo

Tem = existe no nativo · Parcial = existe sem parte do comportamento · Não = ausente.

### Lista de sessões

| Item | PWA | Nativo |
|---|---|---|
| Nome, provider, cwd, branch, tempo | `SessionCard.svelte:301-329` | Parcial — `features/sessions/SessionCard.tsx:33-79` (sem worktree, sem +/−) |
| Chip de estado | `SessionCard.svelte:365-383` | Tem — `StatePill` |
| Chips de plano (barra segmentada), loop, par, motor, limitado | `SessionCard.svelte:324-354`, `PlanBar.svelte` | Não |
| Retomar sessão untracked | `SessionCard.svelte:364-372` | Parcial — só o texto, sem botão |
| Swipe (Git/Excluir) e menu long-press (Renomear/Git/Loop/Excluir) | `SessionCard.svelte:81-124,445-489` | Não |
| Agrupar por servidor/projeto | `SessionList.svelte:369-378`, `sessionListModel.svelte.ts:92,148` | Não — lista plana |
| Filtro/busca | `SessionList.svelte:392-403` | Não |
| Feed "precisa de você" | `AttentionFeed.svelte` | Não |
| Seleção múltipla / broadcast | `SessionList.svelte:363-370,483-506` | Não |
| Drawer de servidores/conta | `SessionList.svelte:274-291` | Não |
| Nova sessão | `SessionList.svelte:508-512` | Tem — `app/create.tsx` |

### Chat

| Item | PWA | Nativo |
|---|---|---|
| Ferramentas consecutivas agrupadas ("Ferramentas: N concluídos") | `ToolGroup.svelte` | Não — `ToolBubble.tsx` uma por linha |
| Pensamento colapsado, busca escondida dentro | `ThinkingBlock.svelte`, `lib/pensamentoTools.svelte.ts` | Não |
| Ações por bolha: hora, copiar, compartilhar, ouvir | `AssistantBubble.svelte:200-217` | Não |
| Arquivo citado com ícone por tipo | `lib/arquivosCitados.ts`, `lib/fileIcons.ts` | Não |
| EditDiff split/unificado | `EditDiff.svelte` | Não (o vendor do Happy tem diff próprio) |
| Tabela → gráfico | `lib/tableChartMount.ts` | Tem — `chat/TableChart.tsx` |
| Anel de contexto no header | `Composer.svelte:1801-1898` | Parcial — `ContextRing.tsx` só na StatusLine/ModelPill |
| Seletor de sessão no título | `NavBar.svelte:105-121` | Não — `ChatHeader.tsx` estático |
| Botão terminal no header | `NavBar.svelte:24-27` | Parcial — dentro do "⋯" (`MoreSheet.tsx`) |
| Rodapé de estatísticas (evento `stats`) | `Chat.svelte:1299`, `Composer.svelte:1989-2024` | Não — evento não escutado |
| Passagem de bastão | `BastaoCard.svelte` | Não |
| Chip de fila / steer | `Composer.svelte` | Tem — `Composer.tsx:337,418-428` |
| Pills modelo/esforço/permissão/estilo de ditado | `Composer.svelte:1789-1958` | Tem — `features/pills/*`, `features/ditado/EstiloPill.tsx` |
| Atalho `/` de comandos | `Composer.svelte:795,829,1564` | Não |

### Tema e material

| Item | PWA | Nativo |
|---|---|---|
| Tokens de cor | `app.css:1-130` | Tem — `packages/core/src/theme.ts` (espelho) |
| `--surface-raised`/`--surface-inset`, sliders Transparência e Solidez | `app.css:115-116,434-445`, `lib/background.ts` | Não — `unistyles.ts:10` fixa `surfaceAlpha: 1` |
| Papel de parede (imagem/textura/aurora) | `lib/background.ts` | Não |
| Cor de acento configurável | `lib/corTema.ts` | Não |
| Tema claro/escuro/sistema com persistência | `lib/theme.ts:7-41` | Parcial — `adaptiveThemes: true`, sem override |
| Idioma segue o sistema | Paraglide `localStorage preferredLanguage baseLocale` | Tem — `net/configureCore.ts` (`expo-localization` → `overwriteGetLocale`); os prints em inglês eram o emulador em en-US |

### Configurações

PWA: 12 telas no `SettingsModal.svelte:109-121` (Geral, Aparência, Diário, Sobre, Máquinas, Contas,
Harnesses, Voz, Notificações, Anexos, Avançado, Motores, Orquestração). Nativo: **nenhuma**; o que
existe por sessão (terminal, atividade, loop, par, arquivos, anexos, limites) vive solto no `MoreSheet`.

## 4. Mudanças em relação à pesquisa de 21/08

- **Manter**: toda a pilha decidida. Instalar o que ficou de fora (`true-sheet`, `sonner-native`,
  `lucide-react-native`, `expo-symbols`, `expo-sharing`) e atualizar expo/reanimated/keyboard-controller.
- **Trocar**: `experimentalBlurMethod` → `blurMethod`.
- **Adicionar**: blur nunca dentro de `Modal` no Android; sheet cross-platform com ≤3 detents;
  `@react-native-menu/menu` só depois de build real em RN 0.86 (plano B `@expo/ui`, não `zeego`);
  conferir uso de `style` em `RNHostView` (`@expo/ui` 57.0.14).

## 5. O que entra na spec de aparência

Ordem sugerida, do que mais muda a percepção pro que menos:

1. **Chat**: tool card curto por chamada (ícone do verbo + 1 linha, estilo Moshi) e agrupamento de 3+
   consecutivas no resumo expansível da PWA; pensamento como linha colapsável; ações por bolha; anel de
   contexto e seletor de sessão no header; rodapé de stats; arquivo citado com ícone; bastão.
2. **Lista**: densidade tipo Vercel — agrupar por servidor/estado, chips de plano/loop/par/motor
   inline, branch inline, prévia ao vivo só quando houver mensagem recente; swipe e menu long-press;
   filtro; feed "precisa de você".
3. **Material**: vidro só em navegação (header, tab/sheet), conteúdo com superfície própria;
   papel de parede + sliders de transparência/solidez espelhando o `background.ts`; cor de acento;
   tema com override (idioma do sistema já funciona).
4. **Configurações**: hub com as telas que fazem sentido no celular (Geral, Aparência, Máquinas,
   Notificações, Voz, Sobre); o resto é desktop.
5. **Componentes nativos**: `true-sheet` pros pickers, menu nativo pra ações de sessão, toast.

Fora deste plano, e já mapeado na pesquisa de 21/08: push (`expo-notifications`), build/distribuição
(EAS, `eas.json`), lojas.

## Fontes

expo.dev/changelog/sdk-57 · github.com/expo/expo (CHANGELOG expo-ui, expo-blur; issues #43743, #42501,
#44165, #47831) · swmansion.com/changelog/react-native-reanimated · github.com/kirillzyusko/
react-native-keyboard-controller/releases · github.com/software-mansion-labs/react-native-streamdown/
releases · github.com/react-native-menu/menu/issues (#1170, #1144, #1201, #1221) · zeego.dev ·
docs.expo.dev (glass-effect, ui/swift-ui/contextmenu, symbols) · unistyl.es/v3/guides/reanimated ·
expo.dev/blog/moving-away-from-expo-vector-icons · github.com/slopus/happy (#59) ·
code.claude.com/docs/en/mobile · simonwillison.net/2026/Feb/25 · buildfastwithai.com (codex mobile) ·
omnara.com · getmoshi.app · vercel.com/changelog/redesigned-deployments-list ·
buildkite.com/docs/pipelines/dashboard-walkthrough · linear.app/changelog/2026-03-12-ui-refresh ·
createwithswift.com (HIG Liquid Glass) · androidauthority.com (Material 3 Expressive) ·
reactlibs.dev (true-sheet) · github.com/gorhom/react-native-bottom-sheet · sonner-native.netlify.app ·
github.com/nandorojo/burnt · versões via `npm view` em 05/09/2026.
