# Happy vendor — tool cards, diff, composer, autocomplete

Origem: https://github.com/slopus/happy.git  
Commit: eb980a5c9eea25b1c145c06cd6241a0a365c2b6d — eb980a5 2026-08-10 feat(server): minimize production logs

Licença: MIT (ver LICENSE).

## O que foi copiado

- `packages/happy-app/sources/components/tools/**` (inclusive `views/**`)
- `packages/happy-app/sources/components/diff/{calculateDiff.ts,DiffView.tsx,PierreDiffView.tsx}`
- `packages/happy-app/sources/components/{CodeView.tsx,CommandView.tsx,MultiTextInput.tsx}`
- `packages/happy-app/sources/components/autocomplete/**`
- `packages/happy-app/sources/utils/{trimIdent,pathUtils,toolErrorParser,toolCommand,codexUnifiedDiff,responsive,thumbhash,toolDisplay,sync,time,truncateForLogs,platform,deviceCalculations}.ts`
- `packages/happy-app/sources/components/layout.ts`
- `packages/happy-app/sources/constants/Typography.ts`

## Shims (não copiados — adaptados ao Hangar)

- `shims/typesMessage.ts` — só `ToolCall`/`Message` que os tools consomem.
- `shims/storageTypes.ts` — `Metadata` + `TodoItemsSchema` (zod).
- `shims/storage.ts` — `useSetting`/`useLocalSetting` lendo `prefs` (mmkv) com defaults; `storage.getState` stub.
- `shims/ops.ts` — `sessionAllow`// no-op; `sessionRipgrep` stub.
- `shims/text.ts` — `t(key, vars)` mapeia para `mobile/src/paraglide/messages` com prefixo `happy_`.
- `hooks/useElapsedTime.ts` — cópia mínima; `hooks/useAttachmentImage.ts` — stub (sem blob).
- `components/markdown/MarkdownView.tsx` — stub (texto puro); `components/AgentInputSuggestionView.tsx` — stubs de tipos/componentes.
- `sync/suggestionCommands.ts` / `sync/suggestionFile.ts` — stubs sem fuse.js (autocomplete não usado na T7).

## O que foi alterado nos arquivos copiados

- `utils/platform.ts` — removido `react-native-device-info`; usa só `Platform.isPad`.
- `hooks/useAttachmentImage.ts` — stub sem decrypt.
- `components/tools/ToolView.tsx`, `PermissionFooter.tsx`, `views/AskUserQuestionView.tsx`, `ExitPlanToolView.tsx`, `FileView.tsx`, `utils/pathUtils.ts` — pequenos fixes de tipagem (`// @ts-nocheck` onde o tipo estrito do vendor não casa com o shim) e `absoluteFillObject`→`absoluteFill`, `isRunningOnMac` etc.
- `components/autocomplete/suggestions.ts` — `// @ts-nocheck` (tipos importados como valor).
- `components/diff/PierreDiffView.tsx` — `theme.dark` → `rt.themeName`.

## Chaves i18n

Todas as `t('…')` usadas pelo vendor foram coletadas (`grep -rho "t('[^']*'"`) e adicionadas como `happy_*` em `messages/pt.json` e `messages/en.json`.

## Dependências acrescentadas (cadeia de imports)

- `zod` — importado por `knownTools.tsx` e `shims/storageTypes`.
- `diff` — importado por `components/diff/calculateDiff.ts`.
- `@expo/vector-icons` — usado pelos tool cards.
- `@pierre/diffs` — usado por `PierreDiffView` (web).

Dependências já presentes e reutilizadas: `react-native-unistyles`, `expo-image`, `react-native-sse` etc. não foram duplicadas.
