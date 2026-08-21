# Pesquisa: ecossistema Expo/React Native para o app mobile do Hangar

Data: 21/08/2026. Fase 0 (research) do trabalho "app mobile Expo". Três varreduras em paralelo
(chat/markdown/listas · UI nativa/glass/sheets · infra SSE/rede/push/monorepo/build), cruzadas com o
npm no mesmo dia. Critério: release nos últimos ~6 meses, New Architecture, TypeScript, Expo SDK atual.

**Estado do ecossistema (conferido no npm em 21/08/2026):** `expo@57.0.15` (20/08) é o estável;
RN 0.87. SDK 55+ é só New Architecture; SDK 56 fez `expo/fetch` virar o `fetch` global e removeu
`expo-av`. Alvo do projeto: **SDK 57**.

## Pilha decidida (resumo)

| Necessidade | Escolha | Motivo em uma linha | Plano B |
|---|---|---|---|
| Lista do chat | `@legendapp/list` 3.3.x | ancora embaixo sem `inverted`/transform; `maintainVisibleContentPosition` ao carregar antigos; `recycleItems={false}` com 120 itens | `@shopify/flash-list` 2.3 (`inverted` voltou em 03/2026) |
| Markdown das bolhas e da prévia | `react-native-enriched-markdown` 1.0.x (Software Mansion) | texto **nativo** (md4c + tree-sitter), GFM tabelas, highlight, links; única que resolve msg longa de verdade | `@ronradtke/react-native-markdown-display` 9.x (puro JS, `rules` custom) |
| Prévia em voo (markdown incompleto) | a mesma, trocando a prop | barato por ser nativo | `react-native-streamdown` 0.2 só se medir jank |
| Diff de Edit/Write | escrever: `diff` (jsdiff) + lista de linhas mono | não existe diff viewer RN vivo; o Claude Code não destaca diff | — |
| Editor de arquivo | `TextInput multiline` mono | highlight em TextInput nativo não existe pra código | — |
| Teclado/composer | `react-native-keyboard-controller` 1.22 + Reanimated 4 + safe-area | `KeyboardStickyView` (barra), `KeyboardChatScrollView`, interactive dismiss | — |
| Estilo/tema | `react-native-unistyles` 3.3 | resolve estilo em C++ no Shadow Tree; **troca de tema/alpha sem re-render** — é o slider de transparência | `StyleSheet` puro (mesma performance inicial, re-render em tema) |
| Vidro | `expo-glass-effect` 57 (iOS 26) + `expo-blur` 57 (iOS <26 e Android) atrás de um `Glass` próprio | mesma cadência do SDK; fallback é obrigatório (regressões medidas no SDK 55) | `@callstack/liquid-glass` 0.8 |
| Sheets | `@lodev09/react-native-true-sheet` 3.11 (ação, composer dentro) + `formSheet` do expo-router (telas com rota) | nativo: detents, blur/glass atrás, teclado nativo | `@gorhom/bottom-sheet` 5.2 (JS) |
| Menu/popover | `@react-native-menu/menu` 2.x (UIMenu/PopupMenu); `@expo/ui` `Popover` no iOS pra conteúdo rico | nativo dos dois lados | `zeego` 3 (casca; release 03/2025) |
| `@expo/ui` 57 | só Picker/Popover/ContextMenu (conteúdo estático) | `Host` instável com lista/ScrollView no SDK 55 | — |
| Push | `expo-notifications` 57 + Expo Push Service | categorias com `textInput`/botões: **responder pergunta da notificação no iOS**; Android não mostra ações (issue #10962) | — |
| KV / segredo | `react-native-mmkv` 4 (Nitro) + `expo-secure-store` | síncrono, JSI; token no Keychain | `AsyncStorage` |
| QR · ditado · clipboard · haptics · imagem | `expo-camera` · `expo-audio` · `expo-clipboard` · `expo-haptics` · `expo-image` | padrão Expo 57 | — |
| Ícones | `lucide-react-native` + `expo-symbols` | mesmo desenho da PWA; SF Symbols onde couber | `@expo/vector-icons` |
| Toast | `sonner-native` 0.27 | JS/Reanimated, combina com vidro | `burnt` (parado 03/2025) |
| HTML/PDF do backend | `react-native-webview` 14 | `source.headers` com o token | — |
| Terminal | `TerminalMirror` por poll (como a PWA); WebView+xterm.js só se precisar de TUI | não existe terminal RN nativo mantido | — |
| Gráfico em resposta | `react-native-gifted-charts` 1.4 | JS+SVG, sem Skia, release semanal | `victory-native` XL |
| **SSE** | **`react-native-sse` 1.2** (XHR, manda `Last-Event-ID`, lê `retry:`, `timeout` = watchdog) | `expo/fetch` tem bugs de abort/chunks no Android; `@ai-sdk/react` não fala nosso protocolo | `react-native-nitro-sse` (24★, cedo) |
| Monorepo | npm workspaces; Metro ≥ SDK 52 detecta sozinho; `packages/core` com `main: ./src/index.ts` (sem build) | Metro não liga `exports` no cliente — `main` é obrigatório | — |
| i18n | um `project.inlang` + `messages/` na raiz; cada app compila seu `src/paraglide` com a própria `--strategy` | `PARAGLIDE_LOCALE` do Svelte vem de localStorage; no RN é `overwriteGetLocale` + `expo-localization` | — |

Descartados por abandono: `react-native-gifted-chat` (manutenção, modelo `IMessage` não cabe tool_use/ask_question), `@flyerhq/react-native-chat-ui` (2022), `stream-chat-react-native` (acoplado ao backend Stream), `@react-native-community/blur` (2024), `react-native-popover-view` (01/2025), `react-native-code-highlighter`/`syntax-highlighter`/`diff-view`/`@rivascva/code-editor` (mortos), `expo-av`, `nativewind` v5 (ainda preview), `@ai-sdk/react` (protocolo próprio, request→response; nosso stream é perene com `state`/`ask_question`).

## O que não tem lib — porte direto do Svelte

Cards `tool_use`/`tool_result` colapsáveis, `AskUserQuestion` com opções, `OptionButtons`, bolhas `pending`/`queued-` e o dedup por id do `Chat.svelte`, `EditDiff`, statusline, pílulas de modelo/esforço por provider, steer. Tudo isso é evento do transcript, não markdown — fica fora do renderer de markdown, em componentes próprios, com a lógica vinda do `@hangar/core`.

## Decisões de infra que entram na spec

1. **SSE**: um stream por sessão aberta na tela (nunca por card). Fechar no `AppState` `background`, reabrir no `active` com `Last-Event-ID`; `timeout: 25_000` como watchdog do `ping` de 10s. `react-native-sse` acumula `responseText` em memória — **reabrir a cada N minutos** (o `Last-Event-ID` retoma). iOS suspende o app em segundos no background; stream em background não existe — é o push que cobre.
2. **Rede local**: iOS `NSAppTransportSecurity.NSAllowsArbitraryLoads: true` + `NSLocalNetworkUsageDescription` (IP privado dispara o prompt de rede local; `100.x` do Tailscale vem por `utun` e não dispara). `NSAllowsLocalNetworking` **não** cobre `192.168.x`. Android: `expo-build-properties` `android.usesCleartextTraffic: true`. Tudo via `app.json`, sem eject.
3. **Push**: app registra `ExpoPushToken` (`getExpoPushTokenAsync({ projectId })`); backend faz `POST https://exp.host/--/api/v2/push/send` (≤100/req, payload ≤4 KiB) com `httpx`, e lê receipts (`DeviceNotRegistered` → apaga token). Android exige FCM V1 (chave de service account no `eas credentials` + `google-services.json`); iOS exige Apple Developer pago — o `eas build` cria a chave APNs sozinho. Deep link: `useLastNotificationResponse()` → `router.push(data.url)`. Categoria `reply` com `textInput` e um botão por opção (iOS ≤4) pra responder `AskUserQuestion` da notificação.
4. **Build sem Mac**: EAS Build Free = 15 iOS + 15 Android/mês, fila lenta, 45 min; Starter US$19/mês. iOS no iPhone: Apple Developer (US$99/ano) → `eas device:create` → `eas build -p ios --profile development` (ad hoc) ou TestFlight interno. Android local em Arch: `jdk17-openjdk`, `android-sdk-cmdline-tools-latest` + `platform-tools` + `emulator` (AUR), `sdkmanager` platform/build-tools/system-image android-36, `avdmanager`, `usermod -aG kvm`, `ANDROID_HOME`/`JAVA_HOME` no fish, `npx expo run:android`. Imagem EAS pro SDK 57: `macos-tahoe-26.5-xcode-26.6`.
5. **Lojas**: Play Console conta pessoal nova ainda exige 12 testadores × 14 dias pra produção (teste interno não exige). App Store: risco é a guideline **2.1** (reviewer não consegue testar um app que depende de servidor próprio) — mitigação: servidor demo público com credenciais nas notas de review durante a janela.
6. **Glass**: exige Xcode 26 (default no EAS); iOS <26 vira `View` (fundo próprio no fallback); `opacity: 0` mata o efeito (animar via `glassEffectStyle.animate`); `UIDesignRequiresCompatibility` desliga tudo e some no Xcode 27; checar `AccessibilityInfo.isReduceTransparencyEnabled()`.

## Fontes principais

expo changelogs SDK 55/56/57 · docs.expo.dev (glass-effect, notifications, push setup/sending/fcm-credentials, build-properties, monorepos, internal-distribution, build limitations/infrastructure, router modals) · github: software-mansion/react-native-enriched-markdown, software-mansion-labs/react-native-streamdown, LegendApp/legend-list, Shopify/flash-list (#1844, PR #2131), lodev09/react-native-true-sheet, gorhom/react-native-bottom-sheet, kirillzyusko/react-native-keyboard-controller, unistyl.es (how it works), efstathiosntonas/react-native-style-libraries-benchmark, binaryminds/react-native-sse, expo/expo issues #33549 #33553 #34772 #10962 #42066 #42904 #32469, mrousavy/react-native-mmkv, gunnartorfis/sonner-native · greatworkeveryone.com (regressão Liquid Glass SDK 55) · ai-sdk.dev (expo, stream-protocol) · paraglidejs.com/monorepo · developer.apple.com (ATS, WWDC20 10110, forums 815919) · support.google.com (12 testers) · versões/datas: `npm view` em 21/08/2026.

## Composer e cards de ferramenta (busca direcionada, 21/08/2026)

**Happy** — github.com/slopus/happy (MIT, 23.4k★, commit 10/08/2026; `packages/happy-app/sources/`).
Cliente mobile open source pra Claude Code e Codex, em Expo 55 / RN 0.83 / unistyles 3.1 /
keyboard-controller 1.21 / reanimated 4 / FlashList 2.3 — a mesma pilha escolhida acima.
O que se copia (com aviso de licença MIT em `mobile/src/vendor/happy/LICENSE`):
- `components/tools/` — `ToolView` (header, status executando/ok/erro, `useElapsedTime`,
  `PermissionFooter`, compacto × card), `ToolSectionView` (colapsável), `ToolError` +
  `utils/toolErrorParser.ts`, `ToolDiffView` (`diff/PierreDiffView`), `knownTools.tsx` com view
  por ferramenta: Bash, Read, Edit, MultiEdit, Write, Glob/Grep/LS, WebFetch/WebSearch, TodoWrite,
  Task (subagente), AskUserQuestion, ExitPlanMode, Skill, MCP genérico, Codex
  (`CodexBash/Patch/Diff/Reasoning`); `*ViewFull` pra tela de detalhe; `CodeView`;
  `utils/toolDisplay.ts`.
- Composer: `MultiTextInput` (+ `.web.tsx`; Enter/Shift+Enter, altura), pasta `autocomplete/`
  (`findActiveWord`, `useActiveSuggestions`, `applySuggestion`, com testes — cobre `/comando` e
  `@arquivo`), `agentInputLayout.ts`, pílulas de modelo/esforço/permissão como referência.
O que diverge e fica nosso: tipos `ToolCall`/`Message` do `@/sync/typesMessage` (escrever adapter
dos eventos `tool_use`/`tool_result` do Hangar), `t()` i18n deles (→ Paraglide), rotas embutidas
no `onPress` (→ nossas), chip "N na fila", steer, ditado, pílulas por provider (Pi/Kimi),
statusline. **Happier** (happier-dev/happier, fork com OpenCode/Kimi/ACP, 1.5k★) é referência
secundária de como tratar Kimi/OpenCode.

Descartados: `@assistant-ui/react-native` 0.1.38 (primitives headless + fila/steer, mas exige o
runtime deles — o SSE do Hangar viraria um `ExternalStoreRuntime` pra ganhar slots vazios),
`@copilotkit/react-native` (composer genérico, generative UI do runtime deles),
`react-native-keyboard-composer` (Android WIP, parado), `InputToolbar` do gifted-chat e
`@kesha-antonov/react-native-chat` (modelo de chat humano), `@stream-io/chat-react-native-ai`
(licença proprietária). Sem versão RN: Vercel AI Elements, ag-ui, prompt-kit, Thesys, Omnara
(só web no repo). Clientes fechados: Conductor, Tonkotsu.

**Veredito:** composer escrito com `TextInput` + keyboard-controller, copiando `MultiTextInput` e
`autocomplete/` do Happy; tool cards = `tools/` + `diff/` + `CodeView` do Happy + adapter de tipos.
Barra visual continua sendo a tela irmã da PWA — o Happy é material, não referência de desenho.
