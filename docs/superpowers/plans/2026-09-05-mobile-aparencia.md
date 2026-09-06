# App nativo: aparência e paridade visual com a PWA — plano de implementação

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Levar o app nativo (`mobile/`) à mesma linguagem visual da PWA na visão celular — lista densa, ferramentas agrupadas com detalhe, pensamento, material de vidro e papel de parede, configurações — usando componentes próprios e o que o nativo faz melhor.

**Architecture:** Regra pura (agrupar ferramentas, pensamento, arquivos citados, ícone por arquivo, cor de acento) vai pro `packages/core` com teste vitest e passa a ser importada pela PWA e pelo nativo. Componente e estilo ficam em `mobile/src`; átomos visuais em `mobile/src/ui/`; preferência de aparelho em MMKV (`mobile/src/stores/prefs.ts`). O vendor do Happy (`mobile/src/vendor/happy`, 71 arquivos) é substituído e apagado na Task 2a. Backend não muda.

**Tech Stack:** Expo SDK 57 / React Native 0.86 (New Architecture), expo-router, react-native-unistyles 3.3, Reanimated 4.6, react-native-gesture-handler 2.32, `@lodev09/react-native-true-sheet`, `lucide-react-native`, `sonner-native`, `expo-blur` / `expo-glass-effect`, `react-native-svg`, `react-native-mmkv`, vitest (mobile e core), TypeScript.

**Spec:** `docs/superpowers/specs/2026-09-05-mobile-aparencia-design.md` (base: `docs/pesquisa-aparencia-mobile-2026-09-05.md`).

## Global Constraints

- Monorepo npm com workspaces: rodar comandos a partir de `/home/jefferson/Projetos/hangar-mobile` (worktree, branch `mobile-expo`). `npm install` sempre na raiz; pacote novo do mobile: `npm install <pkg> -w mobile` (ou `npx expo install <pkg>` dentro de `mobile/`, que escolhe a versão do SDK 57).
- Regra pura em `packages/core/src/<nome>.ts`, exportada em `packages/core/src/index.ts`, com `<nome>.test.ts` ao lado. Dentro do core, importar por caminho relativo (`./types`), nunca `@hangar/core`.
- Componentes em `mobile/src`; todo texto de tela é `m.<chave>()` de `../paraglide/messages`. Chave nova entra em `messages/pt.json` **e** `messages/en.json` no mesmo commit; procurar chave existente antes de criar (`grep '"<palavra>' messages/pt.json`). Depois de mexer em mensagens: `npm run i18n:compile -w mobile`.
- Vidro (`Glass`) só em header, composer, sheets e cabeçalho de grupo. Conteúdo (card, bolha, linha, chip) usa `rgba` sem blur. `BlurView` nunca dentro de `Modal` do RN. Sheet com no máximo 3 detents.
- `expo-blur`: prop `blurMethod` (nunca `experimentalBlurMethod`).
- Idioma do sistema já funciona (`mobile/src/net/configureCore.ts`); não mexer.
- Tokens de cor vêm de `packages/core/src/theme.ts` (espelho do `frontend/src/app.css`); `packages/core/src/theme.test.ts` confere `dark.glass.panelAlpha === 0.86` e `light.glass.panelAlpha === 0.90` — não mudar esses valores.
- Testes: `npm run test -w @hangar/core` e `npm run test -w mobile` **uma vez no fim de cada Task**, nunca em paralelo com build ou emulador. Typecheck: `npm run check -w @hangar/core`, `npm run typecheck -w mobile`, `npm run check -w frontend` quando o core mudou.
- Verificação visual: emulador `hangar` com `-gpu off` e `-no-window`; print via `adb emu screenrecord screenshot /tmp/claude-1000/` (o `screencap` sai branco). Comparar com o print da PWA a 390×844 (`agent-browser set viewport 390 844`, `open http://127.0.0.1:8765/?token=<CP_AUTH_TOKEN do backend/.env>`), mesma sessão e mesmo tema. Julgar opacidade e leiaute, não desfoque (o blur não renderiza com GPU desligada).
- Emulador e Metro: `emulator -avd hangar -no-snapshot -no-audio -no-window -no-boot-anim -gpu off` (destacado com `setsid nohup`); `adb reverse tcp:8765 tcp:8765; adb reverse tcp:8081 tcp:8081`; `npx expo start --dev-client --port 8081` em `mobile/` (destacado). Build nativo só quando entrar lib com código nativo (Task 1): `cd mobile/android && nice -n 19 ./gradlew assembleDebug -PreactNativeArchitectures=x86_64 --max-workers=3` destacado; `adb install -r app/build/outputs/apk/debug/app-debug.apk`.
- Commits pequenos, mensagem em português no padrão do repo (`feat(mobile): …`, `fix(mobile): …`, `refactor(core): …`). Stagear por caminho, nunca `git add -A`.
- Ao terminar um Step, marcar `- [ ]` → `- [x]` neste arquivo.

---

### Task 1: Fundação visual

**Files:**
- Modify: `mobile/package.json` (dependências)
- Modify: `mobile/src/ui/Glass.tsx`
- Create: `mobile/src/ui/Icon.tsx`, `mobile/src/ui/Sheet.tsx`, `mobile/src/ui/Toast.tsx`, `mobile/src/ui/Chip.tsx`, `mobile/src/ui/StateDot.tsx`
- Create: `mobile/src/stores/aparencia.ts`, `mobile/src/stores/aparencia.test.ts`
- Modify: `mobile/app/_layout.tsx`
- Modify: `mobile/src/chat/MoreSheet.tsx`, `mobile/src/chat/ChatHeader.tsx`, `mobile/src/features/pills/PillMenu.tsx`, `mobile/src/features/attachments/Lightbox.tsx`, `mobile/app/login.tsx`
- Modify: `mobile/src/__mocks__/react-native-unistyles.ts` (mock ganha `glass` e `UnistylesRuntime`)

**Interfaces:**
- Produces: `Icon({ name, size?, color? })` com `name` do tipo `keyof typeof import('lucide-react-native')`; `Sheet` (`forwardRef`, `ref.current.present()` / `.dismiss()`, props `sizes?: ('auto'|'medium'|'large')[]` — traduzidas por dentro pros `detents` do true-sheet —, `scrollable?`, `onDismiss?`, `children`); `Glass({ variant?: 'panel'|'modal'|'chrome' })` (já existe, assinatura mantida); `toast.ok(msg)` / `toast.erro(msg)` em `ui/Toast.tsx`; `Chip({ children, tone?: 'neutral'|'accent'|'success'|'warning'|'error', icon? })`; `StateDot({ state })`; store `useAparencia` com `tema: 'system'|'light'|'dark'` e `setTema`.

- [x] **Step 1: Instalar as libs e atualizar as versões**

```bash
cd /home/jefferson/Projetos/hangar-mobile/mobile
npx expo install lucide-react-native @lodev09/react-native-true-sheet sonner-native @react-native-community/slider
npx expo install expo@~57.0.20 react-native-reanimated@~4.6.0 react-native-keyboard-controller@~1.22.4
cd .. && npm install --no-audit --no-fund
```

Confirmar em `mobile/package.json` que as quatro libs novas estão em `dependencies` e que `expo` é `~57.0.20`. `@expo/vector-icons` **ainda não** sai: sai no Step 9, depois que ninguém o importa.

- [x] **Step 2: Rebuild nativo (true-sheet e slider têm código nativo)**

```bash
cd /home/jefferson/Projetos/hangar-mobile/mobile && CI=1 npx expo prebuild --platform android --no-install
cd android && setsid nohup nice -n 19 ./gradlew assembleDebug --console=plain -PreactNativeArchitectures=x86_64 --max-workers=3 > /tmp/claude-1000/gradle-build.log 2>&1 < /dev/null & disown
```

Esperar `BUILD SUCCESSFUL` no log (~10 min). Depois `adb install -r app/build/outputs/apk/debug/app-debug.apk`.

- [x] **Step 3: Teste do store de aparência (tema)**

Criar `mobile/src/stores/aparencia.test.ts`:

```ts
import { describe, it, expect, beforeEach, vi } from 'vitest';

const mem = new Map<string, string>();
vi.mock('react-native-mmkv', () => ({
  createMMKV: () => ({
    getString: (k: string) => mem.get(k),
    set: (k: string, v: string) => void mem.set(k, v),
    remove: (k: string) => void mem.delete(k),
  }),
}));

describe('aparencia', () => {
  beforeEach(() => { mem.clear(); vi.resetModules(); });

  it('tema padrão é system', async () => {
    const { useAparencia } = await import('./aparencia');
    expect(useAparencia.getState().tema).toBe('system');
  });

  it('setTema persiste e valor desconhecido cai em system', async () => {
    const { useAparencia } = await import('./aparencia');
    useAparencia.getState().setTema('dark');
    expect(mem.get('aparencia.tema')).toBe('dark');
    mem.set('aparencia.tema', 'roxo');
    vi.resetModules();
    const { useAparencia: de novo } = await import('./aparencia');
    expect(deNovo.getState().tema).toBe('system');
  });
});
```

- [x] **Step 4: Rodar o teste e ver falhar**

Run: `cd mobile && npx vitest run src/stores/aparencia.test.ts`
Expected: FAIL — `Cannot find module './aparencia'`.

- [x] **Step 5: Implementar `stores/aparencia.ts`**

```ts
import { create } from 'zustand';
import { UnistylesRuntime } from 'react-native-unistyles';
import { prefs } from './prefs';

export type Tema = 'system' | 'light' | 'dark';
const TEMAS: Tema[] = ['system', 'light', 'dark'];
const K = 'aparencia.tema';

function ler(): Tema {
  const v = prefs.getString(K) as Tema | undefined;
  return v && TEMAS.includes(v) ? v : 'system';
}

// Unistyles: `adaptiveThemes` segue o SO; fixar tema exige desligar o adaptativo antes de setTheme.
function aplicar(t: Tema) {
  if (t === 'system') UnistylesRuntime.setAdaptiveThemes(true);
  else { UnistylesRuntime.setAdaptiveThemes(false); UnistylesRuntime.setTheme(t); }
}

export const useAparencia = create<{ tema: Tema; setTema: (t: Tema) => void }>((set) => ({
  tema: ler(),
  setTema: (t) => { prefs.set(K, t); aplicar(t); set({ tema: t }); },
}));

export function aplicarTemaSalvo() { aplicar(useAparencia.getState().tema); }
```

Adicionar ao mock `mobile/src/__mocks__/react-native-unistyles.ts`:

```ts
export const UnistylesRuntime: any = { setAdaptiveThemes: () => {}, setTheme: () => {}, updateTheme: () => {}, themeName: 'dark' };
```

e no `mockTheme.tokens` a chave `glass: { panelRgb: [26,24,29], modalAlpha: 0.93, solidAlpha: 0.94, panelAlpha: 0.86, border: '#333' }` e `panelAlpha: 0.86, surfaceAlpha: 1` no nível do tema.

Em `mobile/app/_layout.tsx`, logo após `configureCore();`: `aplicarTemaSalvo();` (import de `../src/stores/aparencia`).

- [x] **Step 6: Rodar o teste e ver passar**

Run: `cd mobile && npx vitest run src/stores/aparencia.test.ts`
Expected: PASS (2 testes).

- [x] **Step 7: `Icon`, `Chip`, `StateDot`**

`mobile/src/ui/Icon.tsx`:

```tsx
import type { ComponentType } from 'react';
import * as Lucide from 'lucide-react-native';
import { useUnistyles } from 'react-native-unistyles';

export type IconName = keyof typeof Lucide;

export function Icon({ name, size = 18, color }: { name: IconName; size?: number; color?: string }) {
  const { theme } = useUnistyles();
  const Cmp = Lucide[name] as ComponentType<{ size: number; color: string; strokeWidth?: number }>;
  return <Cmp size={size} color={color ?? theme.tokens.text.secondary} strokeWidth={1.75} />;
}
```

`mobile/src/ui/Chip.tsx`:

```tsx
import { Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { Icon, type IconName } from './Icon';

export type Tone = 'neutral' | 'accent' | 'success' | 'warning' | 'error';

export function Chip({ children, tone = 'neutral', icon, mono }: { children: string; tone?: Tone; icon?: IconName; mono?: boolean }) {
  const { theme } = useUnistyles();
  const cor = {
    neutral: theme.tokens.text.secondary,
    accent: theme.tokens.accent.base,
    success: theme.tokens.status.success,
    warning: theme.tokens.status.warning,
    error: theme.tokens.status.error,
  }[tone];
  return (
    <View style={[styles.chip, { borderColor: cor + '55' }]}>
      {icon ? <Icon name={icon} size={11} color={cor} /> : null}
      <Text style={[styles.txt, { color: cor }, mono && styles.mono]} numberOfLines={1}>{children}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  // rgba com o alpha das caixas: acompanha o slider de Solidez, nunca bg cru.
  chip: {
    flexDirection: 'row', alignItems: 'center', gap: 4,
    paddingHorizontal: 6, paddingVertical: 2, borderRadius: theme.base.radius.full, borderWidth: 1,
    backgroundColor: `rgba(${theme.tokens.glass.rgb.join(',')},${theme.surfaceAlpha * 0.5})`,
  },
  txt: { fontSize: theme.base.text.xxs, fontWeight: '600' },
  mono: { fontFamily: theme.base.fontMono, fontWeight: '500' },
}));
```

`mobile/src/ui/StateDot.tsx`:

```tsx
import { View } from 'react-native';
import { stateColors, type State } from '@hangar/core';

export function StateDot({ state, size = 8 }: { state: State; size?: number }) {
  return <View style={{ width: size, height: size, borderRadius: size / 2, backgroundColor: stateColors[state] }} />;
}
```

- [x] **Step 8: `Sheet` e `Toast`**

`mobile/src/ui/Sheet.tsx`:

```tsx
import { forwardRef, type ReactNode } from 'react';
import { TrueSheet, type SheetDetent } from '@lodev09/react-native-true-sheet';
import { useUnistyles } from 'react-native-unistyles';

export type SheetRef = TrueSheet;
type Size = 'auto' | 'medium' | 'large';
// A API do true-sheet 3.11 é `detents: SheetDetent[]` ('auto' | 'peek' | fração 0..1).
const DETENT: Record<Size, SheetDetent> = { auto: 'auto', medium: 0.5, large: 0.92 };

// true-sheet exige New Arch (ligada: app.json newArchEnabled). Teto de 3 detents: Android trunca em 3.
// Fundo do vidro: a sheet é navegação, então leva o modalAlpha; conteúdo dentro dela usa surfaceAlpha.
export const Sheet = forwardRef<TrueSheet, { sizes?: Size[]; children: ReactNode; onDismiss?: () => void; scrollable?: boolean }>(
  function Sheet({ sizes = ['auto'], children, onDismiss, scrollable }, ref) {
    const { theme, rt } = useUnistyles();
    const [r, g, b] = theme.tokens.glass.panelRgb;
    return (
      <TrueSheet
        ref={ref}
        detents={sizes.slice(0, 3).map((s) => DETENT[s])}
        cornerRadius={theme.base.radius.xl}
        backgroundBlur={rt.themeName === 'dark' ? 'dark' : 'light'}
        backgroundColor={`rgba(${r},${g},${b},${theme.tokens.glass.modalAlpha})`}
        grabber
        scrollable={scrollable}
        onDidDismiss={onDismiss ? () => onDismiss() : undefined}
      >
        {children}
      </TrueSheet>
    );
  },
);
```

`mobile/src/ui/Toast.tsx`:

```tsx
import { toast as sonner, Toaster } from 'sonner-native';

export const toast = {
  ok: (msg: string) => sonner.success(msg),
  erro: (msg: string) => sonner.error(msg),
};
export { Toaster };
```

Em `mobile/app/_layout.tsx`, dentro do `GestureHandlerRootView`, depois do `<Stack>`: `<Toaster position="bottom-center" />`.

- [x] **Step 9: Ajustar `Glass` e trocar os `Modal` + `Ionicons`**

`mobile/src/ui/Glass.tsx` — três mudanças, mantendo a assinatura:

```tsx
import { useEffect, useState } from 'react';
import { AccessibilityInfo, Platform, View, type ViewProps } from 'react-native';
// ...imports existentes

function useReduceTransparency() {
  const [on, setOn] = useState(false);
  useEffect(() => {
    AccessibilityInfo.isReduceTransparencyEnabled().then(setOn).catch(() => setOn(false));
    const sub = AccessibilityInfo.addEventListener('reduceTransparencyChanged', setOn);
    return () => sub.remove();
  }, []);
  return on;
}

export function Glass({ variant = 'panel', style, children, ...rest }: Props) {
  const { theme, rt } = useUnistyles();
  const reduzir = useReduceTransparency();
  // ...cálculo de bg igual ao atual
  if (reduzir) {
    return <View style={[styles.box, { backgroundColor: `rgb(${r},${g},${b})` }, style]} {...rest}>{children}</View>;
  }
  if (Platform.OS === 'ios' && isLiquidGlassAvailable()) {
    // key pelo tema: o material do glass-effect não acompanha setColorScheme (expo/expo#43743).
    return <GlassView key={rt.themeName} glassEffectStyle="regular" tintColor={bg} style={[styles.box, style]} {...rest}>{children}</GlassView>;
  }
  return (
    <BlurView intensity={40} blurMethod="dimezisBlurView" tint={rt.themeName === 'dark' ? 'dark' : 'light'} style={[styles.box, style]} {...rest}>
      <View style={[StyleSheet.absoluteFillObject, { backgroundColor: bg }]} />
      {children}
    </BlurView>
  );
}
```

`MoreSheet.tsx`: trocar `Modal` por `Sheet` (`sizes={['auto']}`, `ref` com `useEffect(() => { open ? ref.current?.present() : ref.current?.dismiss(); }, [open])`), e cada `Ionicons name=...` por `<Icon name="…" />` com este mapa: `help-circle-outline`→`CircleHelp`, `pulse-outline`→`Activity`, `repeat-outline`→`Repeat`, `people-outline`→`Users`, `folder-outline`→`Folder`, `terminal-outline`→`Terminal`, `attach-outline`→`Paperclip`, `speedometer-outline`→`Gauge`, `chevron-forward`→`ChevronRight`. O tipo `icon` do `Item` vira `IconName`.
`ChatHeader.tsx`: `chevron-back`→`<Icon name="ChevronLeft" size={24} color={theme.tokens.accent.base} />`, `ellipsis-horizontal`→`<Icon name="Ellipsis" size={20} color={theme.tokens.text.primary} />`.
`PillMenu.tsx` e `Lightbox.tsx`: `Modal` → `Sheet` (`PillMenu`: `sizes={['auto']}`; `Lightbox`: `sizes={['large']}`), mesmo padrão `open`→`present/dismiss`.
`app/login.tsx`: o `Modal` do leitor de QR vira `Sheet` `sizes={['large']}`.

Depois: `grep -rn "from 'react-native'" mobile/src mobile/app | grep -w Modal` e `grep -rn "@expo/vector-icons" mobile/src mobile/app --exclude-dir=vendor` devem voltar vazios (o vendor ainda usa Ionicons; sai na Task 2a).

- [x] **Step 10: Typecheck, testes e prova visual**

Run: `npm run typecheck -w mobile && npm run test -w mobile`
Expected: 0 erros; todos os testes passam (152 anteriores + 2 novos).

Emulador: abrir o app, abrir uma sessão, tocar em "⋯": a `MoreSheet` sobe como sheet nativa com grabber. Print: `adb emu screenrecord screenshot /tmp/claude-1000/`, renomear para `/tmp/claude-1000/t1-moresheet.png`. Conferir que o tema escuro segue o do sistema e que nada ficou com ícone vazio.

- [x] **Step 11: Commit**

```bash
git add mobile/package.json package-lock.json mobile/src/ui mobile/src/stores/aparencia.ts mobile/src/stores/aparencia.test.ts mobile/src/theme/unistyles.ts mobile/app/_layout.tsx mobile/src/chat/MoreSheet.tsx mobile/src/chat/ChatHeader.tsx mobile/src/features/pills/PillMenu.tsx mobile/src/features/attachments/Lightbox.tsx mobile/app/login.tsx mobile/src/__mocks__/react-native-unistyles.ts
git commit -m "feat(mobile): fundação visual — Icon (Lucide), Sheet (true-sheet), Toast, Chip, StateDot, Glass com blurMethod e Reduce Transparency, tema persistido"
```

---

### Task 2a: Chat — ferramentas e fim do vendor

**Files:**
- Create: `packages/core/src/toolGroups.ts`, `packages/core/src/toolGroups.test.ts`
- Modify: `packages/core/src/index.ts`
- Create: `mobile/src/chat/tools/ToolCard.tsx`, `mobile/src/chat/tools/ToolGroup.tsx`, `mobile/src/chat/tools/ToolDetailSheet.tsx`, `mobile/src/chat/tools/EditDiff.tsx`, `mobile/src/chat/tools/toolIcon.ts`, `mobile/src/chat/tools/toolIcon.test.ts`
- Create: `mobile/src/ui/MultilineInput.tsx`
- Modify: `mobile/src/chat/MessageList.tsx`, `mobile/src/chat/Composer.tsx`, `mobile/src/features/files/FileViewer.tsx`, `mobile/src/features/files/FileEditor.tsx`, `mobile/src/theme/unistyles.ts`, `mobile/tsconfig.json`, `mobile/vitest.config.ts`
- Delete: `mobile/src/vendor/happy/` (inteiro), `mobile/src/chat/ToolBubble.tsx`, `mobile/src/chat/toolAdapter.ts`, `mobile/src/chat/toolAdapter.test.ts`, `mobile/src/theme/mapHappy.ts`, `mobile/src/types/react-dom-client.d.ts` (só se nada mais o usa)
- Modify: `frontend/src/components/MessageList.svelte` (passa a usar `agruparConversa` do core)

**Interfaces:**
- Produces (core): `type ItemConversa = { type: 'event'; id; ev } | { type: 'tool'; id; ev } | { type: 'group'; id; tools: ChatEvent[] } | { type: 'pensamento'; id; eventos: ChatEvent[] }`; `agruparConversa(eventos: ChatEvent[], opts: { entraNoPensamento: (nome?: string|null) => boolean; groupMin?: number }): ItemConversa[]`.
- Produces (mobile): `ToolCard({ use, result, onPress })`, `ToolGroup({ tools, resultOf })`, `ToolDetailSheet` (ref, `abrir(use, result)`), `EditDiff({ oldText, newText, modo?: 'unificado'|'lado' })`, `MultilineInput` (`forwardRef<TextInput>`, props do `TextInput` + `maxHeight?`), `toolIcon(nome): IconName`.
- Consumes: `toolPhase`, `toolGroupLabel`, `toolGroupCounts`, `summarizeToolInput`, `computeEditDiff`, `extractEdits`, `extractFilePath` do core; `Icon`, `Sheet`, `Chip` da Task 1.

- [x] **Step 1: Teste da regra de agrupar (core)**

`packages/core/src/toolGroups.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { agruparConversa } from './toolGroups';
import type { ChatEvent } from './types';

const ev = (kind: ChatEvent['kind'], id: string, extra: Partial<ChatEvent> = {}): ChatEvent => ({ kind, id, ...extra });
const tool = (id: string, name = 'Bash') => ev('tool_use', id, { tool_name: name, tool_use_id: id });
const semPensamento = () => false;
const soBusca = (n?: string | null) => n === 'WebSearch' || n === 'WebFetch' || n === 'ToolSearch';

describe('agruparConversa', () => {
  it('1 ou 2 ferramentas seguidas ficam soltas; 3+ viram grupo', () => {
    const a = agruparConversa([tool('a'), tool('b'), ev('assistant_msg', 'm', { text: 'x' })], { entraNoPensamento: semPensamento });
    expect(a.map((i) => i.type)).toEqual(['tool', 'tool', 'event']);
    const b = agruparConversa([tool('a'), tool('b'), tool('c')], { entraNoPensamento: semPensamento });
    expect(b).toEqual([{ type: 'group', id: 'g-a', tools: [tool('a'), tool('b'), tool('c')] }]);
  });

  it('tool_result nunca vira item', () => {
    const r = agruparConversa([tool('a'), ev('tool_result', 'r', { tool_use_id: 'a' })], { entraNoPensamento: semPensamento });
    expect(r.map((i) => i.id)).toEqual(['a']);
  });

  it('pensamentos consecutivos e a busca entre eles viram um bloco; Bash fecha o bloco', () => {
    const r = agruparConversa(
      [ev('thinking', 'p1', { text: 'hm' }), tool('s', 'WebSearch'), ev('thinking', 'p2'), tool('b'), ev('thinking', 'p3')],
      { entraNoPensamento: soBusca },
    );
    expect(r.map((i) => [i.type, i.id])).toEqual([['pensamento', 'p-p1'], ['tool', 'b'], ['pensamento', 'p-p3']]);
    expect((r[0] as { eventos: ChatEvent[] }).eventos.map((e) => e.id)).toEqual(['p1', 's', 'p2']);
  });

  it('busca sem pensamento aberto antes continua card solto', () => {
    const r = agruparConversa([tool('s', 'WebSearch')], { entraNoPensamento: soBusca });
    expect(r[0].type).toBe('tool');
  });

  it('ids repetidos ganham sufixo em vez de derrubar a lista', () => {
    const r = agruparConversa([ev('user_msg', 'q', { text: 'a' }), ev('user_msg', 'q', { text: 'b' })], { entraNoPensamento: semPensamento });
    expect(new Set(r.map((i) => i.id)).size).toBe(2);
  });
});
```

- [x] **Step 2: Rodar e ver falhar**

Run: `npx vitest run src/toolGroups.test.ts --root packages/core`
Expected: FAIL — módulo não existe.

- [x] **Step 3: Implementar `packages/core/src/toolGroups.ts`**

Porte de `frontend/src/components/MessageList.svelte:213-283` sem a parte de `tasks` (a cápsula de tarefas é da PWA; o mobile não a tem) e com `entraNoPensamento` injetado:

```ts
import type { ChatEvent } from './types';

export type ItemConversa =
  | { type: 'event'; id: string; ev: ChatEvent }
  | { type: 'tool'; id: string; ev: ChatEvent }
  | { type: 'group'; id: string; tools: ChatEvent[] }
  | { type: 'pensamento'; id: string; eventos: ChatEvent[] };

export interface OpcoesAgrupar {
  /** Esta chamada, feita no meio do raciocínio, some dentro do bloco de pensamento? */
  entraNoPensamento: (nome?: string | null) => boolean;
  /** A partir de quantas chamadas seguidas vira grupo. 1–2 ficam soltas: não é bagunça. */
  groupMin?: number;
}

// Chave repetida derruba lista keyed (Svelte lança; FlatList duplica). Sufixo determinístico.
function chavesUnicas(ids: string[]): string[] {
  const vistos = new Map<string, number>();
  return ids.map((id) => {
    const n = vistos.get(id) ?? 0;
    vistos.set(id, n + 1);
    return n === 0 ? id : `${id}~${n}`;
  });
}

export function agruparConversa(eventos: ChatEvent[], opts: OpcoesAgrupar): ItemConversa[] {
  const groupMin = opts.groupMin ?? 3;
  const items: ItemConversa[] = [];
  let run: ChatEvent[] = [];
  const flush = () => {
    if (run.length >= groupMin) items.push({ type: 'group', id: `g-${run[0].id}`, tools: run });
    else for (const t of run) items.push({ type: 'tool', id: t.id, ev: t });
    run = [];
  };
  let pens: ChatEvent[] = [];
  const flushPens = () => {
    if (pens.length) items.push({ type: 'pensamento', id: `p-${pens[0].id}`, eventos: pens });
    pens = [];
  };
  for (const ev of eventos) {
    if (ev.kind === 'tool_result') continue;
    if (ev.kind === 'thinking') { flush(); pens.push(ev); continue; }
    if (pens.length && ev.kind === 'tool_use' && opts.entraNoPensamento(ev.tool_name)) { pens.push(ev); continue; }
    flushPens();
    if (ev.kind === 'tool_use') { run.push(ev); continue; }
    flush();
    items.push({ type: 'event', id: ev.id, ev });
  }
  flush();
  flushPens();
  const chaves = chavesUnicas(items.map((i) => i.id));
  return items.map((i, n) => (chaves[n] === i.id ? i : { ...i, id: chaves[n] }));
}
```

Em `packages/core/src/index.ts`: `export * from './toolGroups';`.

- [x] **Step 4: Rodar e ver passar; ligar a PWA na regra do core**

Run: `npx vitest run src/toolGroups.test.ts --root packages/core` → PASS (5).

Em `frontend/src/components/MessageList.svelte`, trocar o laço de `renderItems` (linhas 236-283) por:

```ts
const ehTask = (ev: ChatEvent) => taskRows.ativo && ev.kind === 'tool_use' && EH_TASK(ev.tool_name);
// Âncora da cápsula: o evento NÃO-task imediatamente anterior à última chamada de tarefa. A
// cápsula entra logo depois do item que contém esse evento (ou no topo, se não houver).
let ancora: string | null = null;
for (let i = visibleEvents.length - 1; i >= 0; i--) {
  if (!ehTask(visibleEvents[i])) continue;
  for (let j = i - 1; j >= 0; j--) if (!ehTask(visibleEvents[j]) && visibleEvents[j].kind !== 'tool_result') { ancora = visibleEvents[j].id; break; }
  break;
}
const base: RenderItem[] = agruparConversa(visibleEvents.filter((ev) => !ehTask(ev)), { entraNoPensamento });
if (taskRows.ativo && tarefas.length) {
  const contem = (it: RenderItem) => it.type === 'event' || it.type === 'tool' ? it.id === ancora : it.type === 'group' ? it.tools.some((t) => t.id === ancora) : it.type === 'pensamento' ? it.eventos.some((e) => e.id === ancora) : false;
  const pos = ancora ? base.findIndex(contem) : -1;
  base.splice(pos + 1, 0, { type: 'tasks', id: 'tasks-vivas' });
}
return base;
```

Manter o tipo `RenderItem` como `ItemConversa | { type: 'tasks'; id: string }` e apagar `chavesUnicas` local se só o laço antigo a usava (o core já garante id único). Rodar `npm run check -w frontend` e `npx vitest run src/screens/Chat.test.ts src/components/MessageList.test.ts --root frontend` (o que existir).

- [x] **Step 5: Teste do ícone por ferramenta (mobile)**

`mobile/src/chat/tools/toolIcon.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { toolIcon } from './toolIcon';

describe('toolIcon', () => {
  it('mapeia as ferramentas conhecidas e cai em Wrench', () => {
    expect(toolIcon('Bash')).toBe('Terminal');
    expect(toolIcon('Read')).toBe('Eye');
    expect(toolIcon('Edit')).toBe('Pencil');
    expect(toolIcon('Write')).toBe('FilePlus');
    expect(toolIcon('Grep')).toBe('Search');
    expect(toolIcon('WebSearch')).toBe('Globe');
    expect(toolIcon('Agent')).toBe('Bot');
    expect(toolIcon('Qualquer')).toBe('Wrench');
    expect(toolIcon(null)).toBe('Wrench');
  });
});
```

`mobile/src/chat/tools/toolIcon.ts`:

```ts
import type { IconName } from '../../ui/Icon';

const MAPA: Record<string, IconName> = {
  Bash: 'Terminal', Read: 'Eye', Edit: 'Pencil', MultiEdit: 'Pencil', Write: 'FilePlus', NotebookEdit: 'Pencil',
  Grep: 'Search', Glob: 'FolderSearch', WebSearch: 'Globe', WebFetch: 'Globe', ToolSearch: 'Puzzle',
  Agent: 'Bot', Task: 'Bot', TaskCreate: 'ListTodo', TaskUpdate: 'ListTodo', AskUserQuestion: 'CircleHelp',
  Skill: 'Sparkles', Workflow: 'Workflow',
};
export function toolIcon(nome?: string | null): IconName {
  return (nome && MAPA[nome]) || 'Wrench';
}
```

Run: `cd mobile && npx vitest run src/chat/tools/toolIcon.test.ts` → PASS.

- [x] **Step 6: `ToolCard`**

`mobile/src/chat/tools/ToolCard.tsx`:

```tsx
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { summarizeToolInput, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { toolIcon } from './toolIcon';
import * as m from '../../paraglide/messages';

function duracao(use: ChatEvent, result?: ChatEvent | null): string | null {
  if (!use.ts || !result?.ts) return null;
  const s = Math.max(0, result.ts - use.ts);
  return s < 60 ? `${s.toFixed(s < 10 ? 1 : 0)}s` : `${Math.floor(s / 60)}m${Math.round(s % 60)}s`;
}

// Card curto (estilo Moshi): ícone do verbo + 1 linha + duração/estado. Toque abre o detalhe.
// `semNome`: dentro de um grupo do mesmo tipo o nome já está no cabeçalho.
export function ToolCard({ use, result, onPress, semNome }: { use: ChatEvent; result?: ChatEvent | null; onPress: () => void; semNome?: boolean }) {
  const { theme } = useUnistyles();
  const fase = toolPhase(result ?? null);
  const resumo = summarizeToolInput(use.tool_name, use.tool_input);
  const cor = fase === 'error' ? theme.tokens.status.error : fase === 'pending' ? theme.tokens.accent.base : theme.tokens.text.muted;
  return (
    <Pressable onPress={onPress} style={({ pressed }) => [styles.card, pressed && { opacity: 0.7 }]} accessibilityRole="button" accessibilityLabel={`${use.tool_name ?? m.formato_tool_generico()}: ${resumo}`}>
      <Icon name={toolIcon(use.tool_name)} size={15} color={cor} />
      {!semNome ? <Text style={[styles.nome, { color: theme.tokens.text.secondary }]}>{use.tool_name}</Text> : null}
      <Text style={[styles.resumo, { color: theme.tokens.text.primary }]} numberOfLines={1}>{resumo}</Text>
      <Text style={[styles.dir, { color: cor }]}>{fase === 'pending' ? '…' : fase === 'error' ? '!' : duracao(use, result) ?? ''}</Text>
    </Pressable>
  );
}

const styles = StyleSheet.create((theme) => ({
  card: {
    flexDirection: 'row', alignItems: 'center', gap: 8, paddingHorizontal: 10, paddingVertical: 7,
    borderRadius: theme.base.radius.sm,
    backgroundColor: `rgba(${theme.tokens.glass.rgb.join(',')},${theme.surfaceAlpha * 0.6})`,
  },
  nome: { fontSize: theme.base.text.xs, fontWeight: '600' },
  resumo: { flex: 1, fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
  dir: { fontSize: theme.base.text.xxs, fontFamily: theme.base.fontMono, minWidth: 28, textAlign: 'right' },
}));
```

- [x] **Step 7: `ToolGroup`**

`mobile/src/chat/tools/ToolGroup.tsx`:

```tsx
import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { toolGroupCounts, toolGroupLabel, toolPhase, type ChatEvent } from '@hangar/core';
import { Icon } from '../../ui/Icon';
import { ToolCard } from './ToolCard';
import * as m from '../../paraglide/messages';

// Burst de 3+ chamadas = UMA linha ("Ferramentas: 2 rodando • 3 concluídos"). Aberto, lista os
// ToolCards; a chamada VIVA aparece sob o cabeçalho mesmo fechado, senão o painel escondia o agora.
export function ToolGroup({ tools, resultOf, onAbrir }: { tools: ChatEvent[]; resultOf: (t: ChatEvent) => ChatEvent | null; onAbrir: (use: ChatEvent, result: ChatEvent | null) => void }) {
  const { theme } = useUnistyles();
  const [aberto, setAberto] = useState(false);
  const fases = tools.map((t) => toolPhase(resultOf(t)));
  const label = toolGroupLabel(tools.map((t) => t.tool_name));
  const mixed = label === m.lista_ferramentas();
  const erro = fases.includes('error');
  const viva = tools.find((t, i) => fases[i] === 'pending');
  return (
    <View style={styles.wrap}>
      <Pressable onPress={() => setAberto((v) => !v)} style={styles.head} accessibilityRole="button" accessibilityState={{ expanded: aberto }}>
        <Icon name={aberto ? 'ChevronDown' : 'ChevronRight'} size={14} color={theme.tokens.text.muted} />
        <Text style={[styles.label, { color: erro ? theme.tokens.status.error : theme.tokens.text.secondary }]}>{label}</Text>
        <Text style={[styles.counts, { color: theme.tokens.text.muted }]} numberOfLines={1}>{toolGroupCounts(fases)}</Text>
      </Pressable>
      {!aberto && viva ? <ToolCard use={viva} result={null} semNome={!mixed} onPress={() => onAbrir(viva, null)} /> : null}
      {aberto ? tools.map((t) => <ToolCard key={t.id} use={t} result={resultOf(t)} semNome={!mixed} onPress={() => onAbrir(t, resultOf(t))} />) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { gap: 4 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingVertical: 6, paddingHorizontal: 4 },
  label: { fontSize: theme.base.text.xs, fontWeight: '600' },
  counts: { fontSize: theme.base.text.xxs, flex: 1 },
}));
```

- [x] **Step 8: `EditDiff` (RN) sobre o `computeEditDiff` do core**

O `EditDiff` do core (`packages/core/src/editdiff.ts:17-22`) tem `ops: OpLine[]` (unificado), `rows: SplitRow[]` (`left`/`right`: `{ num, text } | null`), `add`, `del`. Cores como no `EditDiff.svelte`: adição em `status.success` a 18% de alpha, remoção em `status.error` a 18%. `mobile/src/chat/tools/EditDiff.tsx`:

```tsx
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { computeEditDiff } from '@hangar/core';

export function EditDiff({ oldText, newText, modo = 'unificado' }: { oldText: string; newText: string; modo?: 'unificado' | 'lado' }) {
  const { theme } = useUnistyles();
  const d = computeEditDiff(oldText, newText);
  const bg = (op: 'ctx' | 'del' | 'add') => op === 'add' ? theme.tokens.status.success + '2e' : op === 'del' ? theme.tokens.status.error + '2e' : 'transparent';
  if (modo === 'lado') {
    return (
      <ScrollView horizontal>
        <View>
          {d.rows.map((r, i) => (
            <View key={i} style={styles.row}>
              <Text style={[styles.cell, { backgroundColor: r.left ? bg(r.left.op) : 'transparent' }]}>{r.left ? `${r.left.num} ${r.left.text}` : ''}</Text>
              <Text style={[styles.cell, { backgroundColor: r.right ? bg(r.right.op) : 'transparent' }]}>{r.right ? `${r.right.num} ${r.right.text}` : ''}</Text>
            </View>
          ))}
        </View>
      </ScrollView>
    );
  }
  return (
    <View>
      {d.ops.map((l, i) => (
        <Text key={i} style={[styles.line, { backgroundColor: bg(l.op), color: theme.tokens.text.primary }]}>
          {l.op === 'add' ? '+' : l.op === 'del' ? '−' : ' '} {l.text}
        </Text>
      ))}
      <Text style={[styles.line, { color: theme.tokens.text.muted }]}>+{d.add} −{d.del}</Text>
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  line: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, paddingHorizontal: 6, paddingVertical: 1 },
  row: { flexDirection: 'row' },
  cell: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, minWidth: 180, paddingHorizontal: 6, color: '#ccc' },
}));
```

Não há flag de truncamento no tipo: acima de `MYERS_MAX_PRODUCT` o core já devolve del+add em bloco.

- [x] **Step 9: `ToolDetailSheet`**

`mobile/src/chat/tools/ToolDetailSheet.tsx`:

```tsx
import { forwardRef, useImperativeHandle, useRef, useState } from 'react';
import { ScrollView, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { extractEdits, extractFilePath, summarizeToolInput, toolPhase, type ChatEvent } from '@hangar/core';
import { Sheet, type SheetRef } from '../../ui/Sheet';
import { Icon } from '../../ui/Icon';
import { EditDiff } from './EditDiff';
import { toolIcon } from './toolIcon';
import * as m from '../../paraglide/messages';

export interface ToolDetailHandle { abrir: (use: ChatEvent, result: ChatEvent | null) => void }

export const ToolDetailSheet = forwardRef<ToolDetailHandle>(function ToolDetailSheet(_p, ref) {
  const { theme } = useUnistyles();
  const sheet = useRef<SheetRef>(null);
  const [par, setPar] = useState<{ use: ChatEvent; result: ChatEvent | null } | null>(null);
  useImperativeHandle(ref, () => ({ abrir: (use, result) => { setPar({ use, result }); sheet.current?.present(); } }));
  const use = par?.use; const result = par?.result ?? null;
  const edits = use ? extractEdits(use.tool_name, use.tool_input) : null;
  const fase = toolPhase(result);
  return (
    <Sheet ref={sheet} sizes={['medium', 'large']}>
      {use ? (
        <View style={styles.body}>
          <View style={styles.head}>
            <Icon name={toolIcon(use.tool_name)} size={18} />
            <Text style={[styles.titulo, { color: theme.tokens.text.primary }]} numberOfLines={2}>{use.tool_name} · {summarizeToolInput(use.tool_name, use.tool_input)}</Text>
          </View>
          <ScrollView style={styles.scroll}>
            {edits ? edits.map((e, i) => <View key={i}><Text style={[styles.arquivo, { color: theme.tokens.text.secondary }]}>{extractFilePath(use.tool_input)}</Text><EditDiff oldText={e.oldText} newText={e.newText} /></View>) : null}
            {!edits && typeof use.tool_input?.['command'] === 'string' ? <Text style={[styles.mono, { color: theme.tokens.accent.base }]} selectable>$ {String(use.tool_input['command'])}</Text> : null}
            {fase === 'pending' ? <Text style={[styles.mono, { color: theme.tokens.text.muted }]}>{m.formato_rodando({ n: 1 })}</Text> : null}
            {result?.result ? <Text style={[styles.mono, { color: fase === 'error' ? theme.tokens.status.error : theme.tokens.text.primary }]} selectable>{result.result}</Text> : null}
            {fase === 'done' && !result?.result && !edits ? <Text style={[styles.mono, { color: theme.tokens.text.muted }]}>{m.formato_concluido_1({ n: 1 })}</Text> : null}
          </ScrollView>
        </View>
      ) : null}
    </Sheet>
  );
});

const styles = StyleSheet.create((theme) => ({
  body: { padding: theme.base.space[4], gap: theme.base.space[3], maxHeight: '100%' },
  head: { flexDirection: 'row', alignItems: 'center', gap: 8 },
  titulo: { flex: 1, fontSize: theme.base.text.sm, fontWeight: '600' },
  scroll: { flexGrow: 0 },
  arquivo: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xxs, marginBottom: 4 },
  mono: { fontFamily: theme.base.fontMono, fontSize: theme.base.text.xs, lineHeight: 18 },
}));
```

Erro de carga não existe aqui: o resultado já está no evento. O que pode faltar é `result` (chamada viva) — e aí a sheet diz "1 rodando", não fica vazia.

- [x] **Step 10: `MessageList` usa `agruparConversa`, `ToolCard`, `ToolGroup`, `ToolDetailSheet`**

Em `mobile/src/chat/MessageList.tsx`: remover `pairTools`/`toToolCall`/`ToolBubble`; construir `const results = useMemo(() => { const m = new Map<string, ChatEvent>(); for (const e of events) if (e.kind === 'tool_result' && e.tool_use_id) m.set(e.tool_use_id, e); return m; }, [events]);` e `const itens = useMemo(() => agruparConversa(events, { entraNoPensamento: () => false }), [events]);` (o pensamento entra na Task 2b; até lá `thinking` vira item `pensamento` renderizado como `null`). `data={itens}`, `keyExtractor={(i) => i.id}`; no `renderItem`: `event` → bolha (user/assistant como hoje), `tool` → `<ToolCard use={item.ev} result={results.get(item.ev.tool_use_id ?? '') ?? null} onPress={() => detail.current?.abrir(item.ev, results.get(...) ?? null)} />`, `group` → `<ToolGroup tools={item.tools} resultOf={(t) => results.get(t.tool_use_id ?? '') ?? null} onAbrir={(u, r) => detail.current?.abrir(u, r)} />`, `pensamento` → `null`. Montar `<ToolDetailSheet ref={detail} />` uma vez, fora da lista. `estimatedItemSize` fica.

- [x] **Step 11: `MultilineInput` próprio; `Composer`, `FileEditor`, `FileViewer` deixam o vendor**

`mobile/src/ui/MultilineInput.tsx`:

```tsx
import { forwardRef } from 'react';
import { TextInput, type TextInputProps } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';

export const MultilineInput = forwardRef<TextInput, TextInputProps & { maxHeight?: number; mono?: boolean }>(
  function MultilineInput({ maxHeight = 120, mono, style, ...rest }, ref) {
    const { theme } = useUnistyles();
    return (
      <TextInput
        ref={ref}
        multiline
        scrollEnabled
        placeholderTextColor={theme.tokens.text.muted}
        style={[styles.input, { maxHeight, color: theme.tokens.text.primary }, mono && { fontFamily: theme.base.fontMono }, style]}
        {...rest}
      />
    );
  },
);

const styles = StyleSheet.create((theme) => ({
  input: { fontSize: theme.base.text.base, paddingVertical: 8, paddingHorizontal: 10, textAlignVertical: 'top' },
}));
```

`Composer.tsx`: trocar o import de `MultiTextInput` por `MultilineInput`; `inputRef` vira `useRef<TextInput>(null)`; as três chamadas `inputRef.current?.setTextAndSelection(x, {…})` viram `setText(x); setSelection({ start: x.length, end: x.length })` com um `const [selection, setSelection] = useState<{start:number;end:number}|undefined>()` passado como prop `selection={selection}` e zerado (`setSelection(undefined)`) no `onSelectionChange`. O `onKeyPress` fica igual.
`FileEditor.tsx:80`: `<MultilineInput value={texto} onChangeText={setTexto} mono maxHeight={100000} style={{ flex: 1 }} />`.
`FileViewer.tsx:194`: `<EditDiff oldText={diffDoArquivo.original ?? ''} newText={doArquivo?.text ?? ''} />` (import de `../../chat/tools/EditDiff`); o fallback de diff cru abaixo continua.

- [x] **Step 12: Apagar o vendor e o que só existia por ele**

```bash
git rm -r -q mobile/src/vendor/happy mobile/src/chat/ToolBubble.tsx mobile/src/chat/toolAdapter.ts mobile/src/chat/toolAdapter.test.ts mobile/src/theme/mapHappy.ts
```

`mobile/src/theme/unistyles.ts`: remover `happyColors` e a chave `colors` do tema. `mobile/tsconfig.json`: remover `baseUrl` e todo o bloco `paths`. `mobile/vitest.config.ts`: remover os dois aliases `@/…` e `@`. `mobile/babel.config.js`: remover o plugin `module-resolver` inteiro (as 28 entradas `@/…` e `'@': './src/vendor/happy'` — é ele que resolve `@/` no Metro, não o tsconfig); se o plugin só servia ao vendor, `npm uninstall babel-plugin-module-resolver -w mobile`. `grep -rn "react-dom-client" mobile/src` — se só o vendor usava `mobile/src/types/react-dom-client.d.ts`, apagar também. `grep -rn "@expo/vector-icons\|vendor/happy\|'@/" mobile/src mobile/app mobile/babel.config.js mobile/metro.config.js mobile/vitest.config.ts mobile/tsconfig.json` deve voltar vazio; então `npm uninstall @expo/vector-icons -w mobile` (e `@pierre/diffs`, `diff` se `grep -rn "@pierre/diffs\|from 'diff'" mobile/src` voltar vazio).

- [x] **Step 13: Typecheck, testes, prova visual**

Run: `npm run check -w @hangar/core && npm run typecheck -w mobile && npm run check -w frontend`, depois `npm run test -w @hangar/core && npm run test -w mobile` e `npm run test -w frontend`.
Expected: 0 erros; core 426+5, mobile passa (menos os testes do toolAdapter apagados, mais o do toolIcon), frontend 1026.

Emulador (Metro recarrega): abrir `hangar-2`. Comparar com a PWA na mesma sessão: bursts de Bash aparecem como "Ferramentas: N concluídos" fechados; tocar num card abre a sheet com o comando e a saída; um Edit mostra diff colorido. Sheet de arquivos: abrir um arquivo com diff (FileViewer) e o editor (FileEditor). Prints `t2a-chat.png`, `t2a-detalhe.png`, `t2a-arquivos.png`. Comparação cega: subagente fresco recebe `t2a-chat.png` e o print da PWA e diz qual é qual e o que difere em leiaute; até 2 rodadas de ajuste.

- [x] **Step 14: Commit**

```bash
git add packages/core/src/toolGroups.ts packages/core/src/toolGroups.test.ts packages/core/src/index.ts frontend/src/components/MessageList.svelte mobile/src/chat/tools mobile/src/ui/MultilineInput.tsx mobile/src/chat/MessageList.tsx mobile/src/chat/Composer.tsx mobile/src/features/files/FileViewer.tsx mobile/src/features/files/FileEditor.tsx mobile/src/theme/unistyles.ts mobile/tsconfig.json mobile/vitest.config.ts mobile/package.json package-lock.json
git commit -m "feat(mobile): ferramentas com card curto, grupo de 3+ e sheet de detalhe; regra de agrupar no core; vendor do Happy removido"
```

---

### Task 2b: Chat — pensamento, ações por bolha, header, stats, bastão, atalho `/`

**Files:**
- Create: `packages/core/src/pensamento.ts`, `packages/core/src/pensamento.test.ts`, `packages/core/src/arquivosCitados.ts` (movido de `frontend/src/lib/arquivosCitados.ts`), `packages/core/src/fileIcons.ts` + `packages/core/src/fileIcons.generated.ts` (movidos de `frontend/src/lib/`)
- Modify: `packages/core/src/index.ts`; `frontend/src/lib/pensamentoTools.svelte.ts` (passa a delegar pro core); imports da PWA de `arquivosCitados`/`fileIcons` (`grep -rn "lib/arquivosCitados\|lib/fileIcons" frontend/src`)
- Create: `mobile/src/chat/ThinkingBlock.tsx`, `mobile/src/chat/BubbleActions.tsx`, `mobile/src/chat/StatsStrip.tsx`, `mobile/src/chat/StatsStrip.test.ts`, `mobile/src/chat/ArquivoChip.tsx`, `mobile/src/chat/SessionPickerSheet.tsx`, `mobile/src/chat/BastaoSheet.tsx`, `mobile/src/chat/CommandSheet.tsx`
- Modify: `mobile/src/chat/MessageList.tsx`, `mobile/src/chat/AssistantBubble.tsx`, `mobile/src/chat/UserBubble.tsx`, `mobile/src/chat/ChatHeader.tsx`, `mobile/src/chat/MoreSheet.tsx`, `mobile/src/chat/Composer.tsx`, `mobile/src/net/sse.ts`, `mobile/src/stores/chat.ts`, `mobile/src/stores/aparencia.ts`, `mobile/app/s/[server]/[name]/index.tsx`
- Modify: `messages/pt.json`, `messages/en.json` (chaves novas: `bolha_copiar`, `bolha_compartilhar`, `bolha_ouvir`, `bolha_copiado`, `chat_trocar_sessao`, `bastao_titulo`, `bastao_vazio`, `comandos_titulo`)

**Interfaces:**
- Produces (core): `type PensamentoTools = 'nada'|'busca'|'tudo'`; `entraNoPensamento(pref, nome)`, `ehBusca(nome)`, `resumoPensamento(texto): string`; `acumularCitados`, `parseCodePaths`, `estadoVazio` (mesmas assinaturas de hoje); `nomeIcone(nome, isDir, aberta?)`, `svgIcone(...)`.
- Produces (mobile): `useAparencia.pensamentoTools` + `setPensamentoTools`; `useChatStore.stats: StatsEvent | null`; `ChatHeader` ganha props `onTitlePress`, `onTerminal`, `contextPct?: number | null`.
- Consumes: `ItemConversa` (2a), `Sheet`/`Icon`/`Chip` (1), `sintetizarTts`, `ttsAudioUrl`, `getBastaoDossie`, `getCommands`, `parseStatusLine` do core.

- [x] **Step 1: Teste da regra de pensamento (core)**

`packages/core/src/pensamento.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { entraNoPensamento, ehBusca, resumoPensamento } from './pensamento';

describe('pensamento', () => {
  it('busca: só WebSearch/WebFetch/ToolSearch entram; tarefas nunca', () => {
    expect(entraNoPensamento('busca', 'WebSearch')).toBe(true);
    expect(entraNoPensamento('busca', 'ToolSearch')).toBe(true);
    expect(entraNoPensamento('busca', 'Bash')).toBe(false);
    expect(entraNoPensamento('tudo', 'Bash')).toBe(true);
    expect(entraNoPensamento('tudo', 'TaskCreate')).toBe(false);
    expect(entraNoPensamento('nada', 'WebSearch')).toBe(false);
  });
  it('ehBusca', () => { expect(ehBusca('WebFetch')).toBe(true); expect(ehBusca('Read')).toBe(false); });
  it('resumo é a primeira frase curta', () => {
    expect(resumoPensamento('Vou olhar o arquivo. Depois testo.')).toBe('Vou olhar o arquivo.');
    expect(resumoPensamento('x'.repeat(200) + '. fim')).toBe('x'.repeat(200) + '. fim');
  });
});
```

- [x] **Step 2: Rodar e ver falhar** — `npx vitest run src/pensamento.test.ts --root packages/core` → FAIL.

- [x] **Step 3: Implementar `packages/core/src/pensamento.ts`** (porte de `frontend/src/lib/pensamentoTools.svelte.ts` sem o `localStorage`, e do `resumo` do `ThinkingBlock.svelte`):

```ts
export type PensamentoTools = 'nada' | 'busca' | 'tudo';
export const PENSAMENTO_TOOLS: PensamentoTools[] = ['nada', 'busca', 'tudo'];

const BUSCA = new Set(['WebSearch', 'WebFetch']);
const CARREGADOR = 'ToolSearch';
const TAREFA = new Set(['TaskCreate', 'TaskUpdate']);

export function ehBusca(nome?: string | null): boolean {
  return !!nome && (BUSCA.has(nome) || nome === CARREGADOR);
}

export function entraNoPensamento(pref: PensamentoTools, nome?: string | null): boolean {
  if (pref === 'nada') return false;
  if (nome && TAREFA.has(nome)) return false;
  if (pref === 'tudo') return true;
  return ehBusca(nome);
}

export function resumoPensamento(texto: string): string {
  const limpo = texto.replace(/\s+/g, ' ').trim();
  const fim = limpo.search(/[.!?](\s|$)/);
  return fim > 0 && fim < 140 ? limpo.slice(0, fim + 1) : limpo;
}
```

`index.ts`: `export * from './pensamento';`. Em `frontend/src/lib/pensamentoTools.svelte.ts`: apagar `BUSCA`/`CARREGADOR`/`TAREFA` e as duas funções locais; reexportar `export { ehBusca } from '@hangar/core'` e `export function entraNoPensamento(nome?) { return entraNoPensamentoCore(pref, nome); }` (import `entraNoPensamento as entraNoPensamentoCore`). Em `ThinkingBlock.svelte`, `resumo` usa `resumoPensamento`.

- [x] **Step 4: Mover `arquivosCitados` e `fileIcons` pro core**

```bash
git mv frontend/src/lib/arquivosCitados.ts packages/core/src/arquivosCitados.ts
git mv frontend/src/lib/arquivosCitados.test.ts packages/core/src/arquivosCitados.test.ts
git mv frontend/src/lib/fileIcons.ts packages/core/src/fileIcons.ts
git mv frontend/src/lib/fileIcons.generated.ts packages/core/src/fileIcons.generated.ts
git mv frontend/src/lib/fileIcons.test.ts packages/core/src/fileIcons.test.ts
```

Nos arquivos movidos: `import type { ChatEvent } from '@hangar/core'` → `from './types'`. `index.ts`: `export * from './arquivosCitados'; export * from './fileIcons';`. Na PWA, `grep -rln "lib/arquivosCitados\|lib/fileIcons" frontend/src` e trocar cada import por `@hangar/core` (o `fileIcons.lista.json` e `scripts/geraFileIcons.mjs` ficam no frontend; o script passa a escrever em `../packages/core/src/fileIcons.generated.ts` — ajustar o caminho de saída dentro dele). Rodar `npx vitest run src/pensamento.test.ts src/arquivosCitados.test.ts src/fileIcons.test.ts --root packages/core` → PASS; `npm run check -w frontend` → 0 erros.

- [x] **Step 5: Preferência `pensamentoTools` no store de aparência**

Em `mobile/src/stores/aparencia.ts` acrescentar `pensamentoTools: PensamentoTools` (chave MMKV `aparencia.pensamentoTools`, padrão `'busca'`, valor desconhecido cai no padrão) e `setPensamentoTools`. Teste em `aparencia.test.ts`: `it('pensamentoTools padrão busca e persiste', …)` no mesmo padrão dos de tema.

- [x] **Step 6: `ThinkingBlock`**

`mobile/src/chat/ThinkingBlock.tsx`:

```tsx
import { useState } from 'react';
import { Pressable, Text, View } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { ehBusca, resumoPensamento, summarizeToolInput, type ChatEvent } from '@hangar/core';
import { Icon } from '../ui/Icon';
import * as m from '../paraglide/messages';

export function ThinkingBlock({ eventos }: { eventos: ChatEvent[] }) {
  const { theme } = useUnistyles();
  const [aberto, setAberto] = useState(false);
  const pensamentos = eventos.filter((e) => e.kind === 'thinking');
  const chamadas = eventos.filter((e) => e.kind === 'tool_use' && e.tool_name !== 'ToolSearch');
  const soBusca = chamadas.every((e) => ehBusca(e.tool_name));
  const n = chamadas.length;
  const rotulo = n === 0 ? m.pensamento_rotulo() : soBusca ? (n === 1 ? m.pensamento_uma_busca() : m.pensamento_buscas({ n })) : (n === 1 ? m.pensamento_uma_chamada() : m.pensamento_chamadas({ n }));
  const resumo = resumoPensamento(pensamentos[0]?.text ?? '');
  return (
    <View style={styles.wrap}>
      <Pressable onPress={() => setAberto((v) => !v)} style={styles.head} accessibilityRole="button" accessibilityState={{ expanded: aberto }}>
        <Icon name="Brain" size={13} color={theme.tokens.text.muted} />
        <Text style={[styles.rotulo, { color: theme.tokens.text.muted }]}>{rotulo}</Text>
        {!aberto ? <Text style={[styles.resumo, { color: theme.tokens.text.muted }]} numberOfLines={1}>{resumo}</Text> : null}
      </Pressable>
      {aberto ? (
        <View style={styles.corpo}>
          {eventos.map((e) => e.kind === 'thinking'
            ? <Text key={e.id} style={[styles.texto, { color: theme.tokens.text.secondary }]}>{e.text}</Text>
            : e.tool_name === 'ToolSearch' ? null
            : <Text key={e.id} style={[styles.busca, { color: theme.tokens.text.muted }]}>· {e.tool_name}: {summarizeToolInput(e.tool_name, e.tool_input)}</Text>)}
        </View>
      ) : null}
    </View>
  );
}

const styles = StyleSheet.create((theme) => ({
  wrap: { paddingVertical: 2 },
  head: { flexDirection: 'row', alignItems: 'center', gap: 6, paddingVertical: 6, paddingHorizontal: 4 },
  rotulo: { fontSize: theme.base.text.xs, fontStyle: 'italic' },
  resumo: { flex: 1, fontSize: theme.base.text.xs, fontStyle: 'italic' },
  corpo: { gap: 6, paddingLeft: 23, paddingRight: 8, paddingBottom: 6 },
  texto: { fontSize: theme.base.text.sm, lineHeight: 20 },
  busca: { fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
}));
```

`MessageList.tsx`: `agruparConversa(events, { entraNoPensamento: (n) => entraNoPensamento(pref, n) })` com `const pref = useAparencia((s) => s.pensamentoTools)`; item `pensamento` → `<ThinkingBlock eventos={item.eventos} />`.

- [x] **Step 7: Ações por bolha**

`mobile/src/chat/BubbleActions.tsx`:

```tsx
import { Pressable, Share, Text, View } from 'react-native';
import * as Clipboard from 'expo-clipboard';
import { createAudioPlayer } from 'expo-audio';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { sintetizarTts, ttsAudioUrl } from '@hangar/core';
import { Icon } from '../ui/Icon';
import { toast } from '../ui/Toast';
import * as m from '../paraglide/messages';

function hora(ts?: number | null) {
  return ts ? new Date(ts * 1000).toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' }) : '';
}

// Hora + copiar/compartilhar/ouvir. Compartilhar usa o Share do RN (texto puro, sem lib). Ouvir
// chama o mesmo POST /api/tts da PWA e toca a URL devolvida.
export function BubbleActions({ text, ts, ouvir = true }: { text: string; ts?: number | null; ouvir?: boolean }) {
  const { theme } = useUnistyles();
  const c = theme.tokens.text.muted;
  const copiar = async () => { await Clipboard.setStringAsync(text); toast.ok(m.bolha_copiado()); };
  const compartilhar = () => Share.share({ message: text }).catch(() => {});
  const falar = async () => {
    try {
      const r = await sintetizarTts({ text });
      createAudioPlayer(ttsAudioUrl(r.url)).play();
    } catch (e) { toast.erro(e instanceof Error ? e.message : String(e)); }
  };
  return (
    <View style={styles.row}>
      <Text style={[styles.hora, { color: c }]}>{hora(ts)}</Text>
      <Pressable onPress={copiar} hitSlop={8} accessibilityLabel={m.bolha_copiar()}><Icon name="Copy" size={13} color={c} /></Pressable>
      <Pressable onPress={compartilhar} hitSlop={8} accessibilityLabel={m.bolha_compartilhar()}><Icon name="Share2" size={13} color={c} /></Pressable>
      {ouvir ? <Pressable onPress={falar} hitSlop={8} accessibilityLabel={m.bolha_ouvir()}><Icon name="Volume2" size={13} color={c} /></Pressable> : null}
    </View>
  );
}

const styles = StyleSheet.create(() => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: 14, paddingHorizontal: 6, paddingTop: 4 },
  hora: { fontSize: 11, marginRight: 'auto' },
}));
```

`AssistantBubble` e `UserBubble` ganham prop `ts?: number | null` e renderizam `<BubbleActions text={text} ts={ts} ouvir={/*assistant*/ true|false} />` abaixo do conteúdo; `MessageList` passa `ts={item.ev.ts}`. Chaves `bolha_copiar` ("Copiar"/"Copy"), `bolha_compartilhar` ("Compartilhar"/"Share"), `bolha_ouvir` ("Ouvir"/"Listen"), `bolha_copiado` ("Copiado"/"Copied") nos dois json.

- [x] **Step 8: Arquivo citado com ícone**

`mobile/src/chat/ArquivoChip.tsx`:

```tsx
import { Pressable, Text } from 'react-native';
import { SvgXml } from 'react-native-svg';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { svgIcone } from '@hangar/core';

export function ArquivoChip({ caminho, onPress }: { caminho: string; onPress: () => void }) {
  const { theme } = useUnistyles();
  const nome = caminho.split('/').pop() ?? caminho;
  return (
    <Pressable onPress={onPress} style={styles.chip} accessibilityRole="link" accessibilityLabel={caminho}>
      <SvgXml xml={svgIcone(nome, false)} width={14} height={14} />
      <Text style={[styles.txt, { color: theme.tokens.text.primary }]} numberOfLines={1}>{nome}</Text>
    </Pressable>
  );
}
const styles = StyleSheet.create((theme) => ({
  chip: { flexDirection: 'row', alignItems: 'center', gap: 5, paddingHorizontal: 8, paddingVertical: 3, borderRadius: theme.base.radius.full, backgroundColor: `rgba(${theme.tokens.glass.rgb.join(',')},${theme.surfaceAlpha * 0.6})`, alignSelf: 'flex-start' },
  txt: { fontSize: theme.base.text.xs, fontFamily: theme.base.fontMono },
}));
```

Em `AssistantBubble.tsx`, depois do markdown: `parseCodePaths(text).map((p) => <ArquivoChip key={p} caminho={p} onPress={() => router.push(`/s/${serverId}/${name}/files?path=${encodeURIComponent(p)}` as never)} />)` numa `View` com `flexWrap: 'wrap'` (a rota `files` já existe; se ela não lê `path` da query, acrescentar `useLocalSearchParams<{ path?: string }>()` em `app/s/[server]/[name]/files.tsx` e abrir o arquivo quando vier). `AssistantBubble` precisa receber `serverId` (já recebe `sessionName`; acrescentar `serverId`).

- [x] **Step 9: Evento `stats` no SSE, store e `StatsStrip`**

`mobile/src/net/sse.ts:70-71`: acrescentar `type === 'stats'` à lista `isData`. `mobile/src/stores/chat.ts`: campo `stats: StatsEvent | null` (inicial `null`), listener `es.addEventListener('stats', (e) => set({ stats: JSON.parse(e.data) as StatsEvent }))` junto dos outros, e zerar em `reset`.

`mobile/src/chat/StatsStrip.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { linhaStats } from './StatsStrip';

describe('linhaStats', () => {
  it('monta as partes na ordem da PWA e omite o que não veio', () => {
    expect(linhaStats({ turns: 1, steps: 3, in_tok: 1200, out_tok: 300 })).toEqual(['1 turno', '3 chamadas', '↓ 1.2k · ↑ 300 tok']);
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, tok_s: 41.6, cache_pct: 80 })).toContain('~42 tok/s');
    expect(linhaStats({ turns: 2, steps: 1, in_tok: 0, out_tok: 0, cache_pct: 80 })).toContain('cache 80%');
  });
});
```

`mobile/src/chat/StatsStrip.tsx`:

```tsx
import { ScrollView, Text } from 'react-native';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import { abbrevNum, type StatsEvent } from '@hangar/core';
import * as m from '../paraglide/messages';

const fmtDur = (ms: number) => (ms < 60_000 ? `${Math.round(ms / 1000)}s` : `${Math.floor(ms / 60_000)}m${Math.round((ms % 60_000) / 1000)}s`);

export function linhaStats(s: StatsEvent): string[] {
  const p = [s.turns === 1 ? m.stats_turnos_1() : m.stats_turnos({ n: s.turns }), s.steps === 1 ? m.stats_chamadas_1() : m.stats_chamadas({ n: s.steps })];
  if (s.llm_ms) { p.push(m.stats_llm({ t: fmtDur(s.llm_ms) })); if (s.tool_ms) p.push(m.stats_tools({ t: fmtDur(s.tool_ms) })); }
  if (s.tok_s) p.push(m.stats_toks({ n: Math.round(s.tok_s) }));
  if (s.ttft_ms) p.push(m.stats_ttft({ t: fmtDur(s.ttft_ms) }));
  if (s.cache_pct != null) p.push(m.stats_cache({ n: s.cache_pct }));
  p.push(m.stats_io({ i: abbrevNum(s.in_tok), o: abbrevNum(s.out_tok) }));
  return p;
}

export function StatsStrip({ stats }: { stats: StatsEvent | null }) {
  const { theme } = useUnistyles();
  if (!stats) return null;
  return (
    <ScrollView horizontal showsHorizontalScrollIndicator={false} contentContainerStyle={styles.strip} accessibilityLabel={m.stats_faixa_aria()}>
      <Text style={[styles.txt, { color: theme.tokens.text.muted }]}>{linhaStats(stats).join('  ·  ')}</Text>
    </ScrollView>
  );
}
const styles = StyleSheet.create(() => ({ strip: { paddingHorizontal: 12, paddingVertical: 4 }, txt: { fontSize: 11 } }));
```

O teste roda com o mock do paraglide? Não há mock: o `paraglide/messages` compilado devolve pt-BR (locale do teste). Se `m.stats_turnos_1()` vier em inglês no CI, fixar o locale no teste com `overwriteGetLocale(() => 'pt')` de `../paraglide/runtime`. Montar `<StatsStrip stats={useChatStore((s) => s.stats)} />` acima do `Composer` em `app/s/[server]/[name]/index.tsx`.

- [x] **Step 10: `ChatHeader` com anel de contexto, título tocável e terminal**

`ChatHeader.tsx` ganha `onTitlePress: () => void`, `onTerminal: () => void`, `contextPct?: number | null`. O título vira `Pressable` (`accessibilityLabel={m.chat_trocar_sessao()}`) com `<Icon name="ChevronDown" size={14} />` ao lado; antes do "⋯" entra `<Pressable onPress={onTerminal} style={styles.more}><Icon name="Terminal" size={20} /></Pressable>`; à esquerda da pill de estado, `<ContextRing pct={contextPct} />` (a prop é `pct?: number | null`, `mobile/src/chat/ContextRing.tsx:12`). `contextPct` vem de `parseStatusLine(statusLine)?.ctxPct` (`packages/core/src/statusline.ts:15`).

`mobile/src/chat/SessionPickerSheet.tsx`: `Sheet sizes={['medium','large']}` listando `useSessions((s) => s.rows)` ordenadas por `sortSessions`, cada linha `StateDot` + nome + servidor; toque → `router.replace(`/s/${serverId}/${name}`)`. Em `index.tsx`: `onTitlePress` abre essa sheet, `onTerminal` faz `router.push(`/s/${serverId}/${name}/terminal`)`. Chave `chat_trocar_sessao` ("Trocar de sessão"/"Switch session").

- [x] **Step 11: `BastaoSheet` e `CommandSheet`**

`mobile/src/chat/BastaoSheet.tsx`: `Sheet sizes={['large']}`; ao abrir chama `getBastaoDossie(name)`; estado `carregando | erro | texto`; texto renderizado com `EnrichedMarkdownText` e o `mkMarkdownStyle` que `AssistantBubble` exporta; 404 vira `m.bastao_vazio()` ("Esta sessão não recebeu bastão"/"This session received no handoff"); outro erro mostra a mensagem em vermelho e botão "Tentar de novo". Entra como item `{ icon: 'Baton'?? → 'Flag', label: m.bastao_titulo() }` na `MoreSheet` (rota nova `bastao` em `app/s/[server]/[name]/bastao.tsx` registrada no `_layout.tsx` como `formSheet`, igual às irmãs). Chave `bastao_titulo` ("Passagem de bastão"/"Handoff").

`mobile/src/chat/CommandSheet.tsx`: `Sheet sizes={['medium']}`; lista de `getCommands(name)` filtrada pelo texto depois do `/`; toque insere `display + ' '` no composer. Em `Composer.tsx`, `handleChangeText`: se o texto começa com `/` e não tem espaço, abre a sheet e passa o filtro; ao escolher, `setText`. Chave `comandos_titulo` ("Comandos"/"Commands").

- [x] **Step 12: Typecheck, testes, prova visual**

Run: `npm run check -w @hangar/core && npm run typecheck -w mobile && npm run check -w frontend`; `npm run test -w @hangar/core && npm run test -w mobile && npm run test -w frontend`.
Expected: 0 erros, tudo verde (core ganha pensamento 3, arquivosCitados e fileIcons movidos; mobile ganha StatsStrip 1 e aparencia +1).

Emulador em `hangar-2`: bloco "pensou · N buscas" fechado com resumo, abre ao toque; hora e ações sob as bolhas; anel de contexto no header; título abre o seletor; rodapé de stats visível; `/` no composer abre a lista de comandos; MoreSheet → Bastão abre a sheet (com o dossiê ou o aviso de vazio). Prints `t2b-chat.png`, `t2b-pensamento.png`, `t2b-comandos.png`. Comparação cega com a PWA na mesma sessão (até 2 rodadas).

- [x] **Step 13: Commit**

```bash
git add packages/core/src frontend/src/lib/pensamentoTools.svelte.ts frontend/src/components/ThinkingBlock.svelte frontend/src frontend/scripts/geraFileIcons.mjs mobile/src mobile/app messages/pt.json messages/en.json
git commit -m "feat(mobile): pensamento colapsado, ações por bolha, header com contexto e seletor, stats, bastão e atalho /; regras de pensamento e arquivos citados no core"
```

(Conferir com `git status` antes que só entram os arquivos desta Task; se a árvore tiver outra coisa, stagear por caminho.)

---

### Task 3: Lista de sessões

**Files:**
- Create: `packages/core/src/agruparSessoes.ts`, `packages/core/src/agruparSessoes.test.ts`
- Modify: `packages/core/src/index.ts`
- Create: `mobile/src/features/sessions/SessionRow.tsx`, `mobile/src/features/sessions/SessionMenu.tsx`, `mobile/src/features/sessions/AttentionStrip.tsx`, `mobile/src/features/sessions/ServerSheet.tsx`, `mobile/src/features/sessions/demo.test.ts`
- Modify: `mobile/src/features/sessions/SessionList.tsx`, `mobile/app/index.tsx`, `mobile/src/stores/aparencia.ts`
- Delete: `mobile/src/features/sessions/SessionCard.tsx` (substituída por `SessionRow`)
- Modify: `messages/pt.json`, `messages/en.json` (chaves: `lista_filtro_placeholder`, `lista_agrupar_servidor`, `lista_agrupar_projeto`, `lista_agrupar_nao`)

**Interfaces:**
- Produces (core): `agruparSessoes(rows: AggSession[], modo: GroupBy): { id: string; label: string; color: string | null; sessions: AggSession[] }[]` (usa `projectKey`/`projectLabel`/`sortSessions`).
- Produces (mobile): `SessionRow({ session, onPress, onGit, onExcluir, onRenomear, onLoop, onResume })`; `SessionMenu` (menu nativo); `AttentionStrip({ sessions, onOpen })`; `ServerSheet` (ref).
- Consumes: `attentionFeed`, `effectiveGroupBy`, `planBadge`, `loopBadge`, `providerTag`, `relativeTime`, `cwdParts`, `resumeSession`, `deleteSession`, `renameSession` do core; `PlanBar`/`PlanChip` (`features/plan`), `LoopChip` (`chat/`), `Chip`/`StateDot`/`Icon`/`Sheet` (Task 1).

- [x] **Step 1: Teste de `agruparSessoes` (core)**

```ts
import { describe, it, expect } from 'vitest';
import { agruparSessoes } from './agruparSessoes';
import type { AggSession } from './types';

const s = (name: string, serverId: string, cwd: string, state: AggSession['state'] = 'idle'): AggSession =>
  ({ name, serverId, serverLabel: serverId, serverColor: '#123', cwd, state } as AggSession);

describe('agruparSessoes', () => {
  const rows = [s('b', 'A', '/x/repo1'), s('a', 'A', '/x/repo1', 'awaiting_input'), s('c', 'B', '/y/repo2')];
  it('none = um grupo só, ordenado por sortSessions', () => {
    const g = agruparSessoes(rows, 'none');
    expect(g).toHaveLength(1);
    expect(g[0].sessions.map((x) => x.name)).toEqual(['a', 'b', 'c']);
  });
  it('server = um grupo por servidor, cor do servidor', () => {
    const g = agruparSessoes(rows, 'server');
    expect(g.map((x) => [x.id, x.sessions.length])).toEqual([['A', 2], ['B', 1]]);
    expect(g[0].color).toBe('#123');
  });
  it('project = um grupo por cwd, rótulo é o basename', () => {
    const g = agruparSessoes(rows, 'project');
    expect(g.map((x) => x.label)).toEqual(['repo1', 'repo2']);
  });
});
```

- [x] **Step 2: Rodar e ver falhar** — `npx vitest run src/agruparSessoes.test.ts --root packages/core` → FAIL.

- [x] **Step 3: Implementar `packages/core/src/agruparSessoes.ts`**

```ts
import type { AggSession } from './types';
import { projectKey, projectLabel, sortSessions, type GroupBy } from './format';

export interface GrupoSessoes { id: string; label: string; color: string | null; sessions: AggSession[] }

export function agruparSessoes(rows: AggSession[], modo: GroupBy): GrupoSessoes[] {
  const ordenadas = sortSessions(rows) as AggSession[];
  if (modo === 'none') return [{ id: 'todas', label: '', color: null, sessions: ordenadas }];
  const por = new Map<string, GrupoSessoes>();
  for (const r of ordenadas) {
    const id = modo === 'server' ? r.serverId : projectKey(r.cwd);
    const g = por.get(id) ?? { id, label: modo === 'server' ? r.serverLabel : projectLabel(r.cwd), color: modo === 'server' ? r.serverColor : null, sessions: [] };
    g.sessions.push(r);
    por.set(id, g);
  }
  return [...por.values()];
}
```

`index.ts`: `export * from './agruparSessoes';`. Run → PASS (3).

- [x] **Step 4: Preferência de agrupamento**

`stores/aparencia.ts`: `agrupar: GroupBy` (MMKV `lista.agrupar`, padrão `'server'`, aceita `none|server|project`) + `setAgrupar`. Teste no `aparencia.test.ts`.

- [x] **Step 5: `SessionRow` (linha densa) com swipe**

`mobile/src/features/sessions/SessionRow.tsx` — porte de `frontend/src/components/SessionCard.svelte:296-390`:

```tsx
import { Pressable, Text, View } from 'react-native';
import ReanimatedSwipeable from 'react-native-gesture-handler/ReanimatedSwipeable';
import { StyleSheet, useUnistyles } from 'react-native-unistyles';
import * as Haptics from 'expo-haptics';
// LOOP_TONE_COLOR do core é CSS var (`var(--accent)`) — não serve em RN; o tom vira `Chip tone`.
import { cwdParts, loopBadge, planBadge, providerTag, relativeTime, untrackedReason, type AggSession } from '@hangar/core';
import { Chip } from '../../ui/Chip';
import { StateDot } from '../../ui/StateDot';
import { Icon } from '../../ui/Icon';
import { PlanBar } from '../plan/PlanBar';
import * as m from '../../paraglide/messages';

interface Props {
  session: AggSession;
  mostrarServidor: boolean;   // false quando a lista já está agrupada por servidor
  onPress: () => void;
  onLongPress: () => void;
  onGit: () => void;
  onExcluir: () => void;
  onResume: () => void;
}

export function SessionRow({ session: s, mostrarServidor, onPress, onLongPress, onGit, onExcluir, onResume }: Props) {
  const { theme } = useUnistyles();
  const untracked = s.tracked === false;
  const cwd = cwdParts(s.cwd);
  const showCwd = !!s.cwd && cwd.base.toLowerCase() !== s.name.toLowerCase();
  const loop = loopBadge(s.loop_status, s.loop_iter, s.loop_max);
  const plan = planBadge(s);
  const sub = s.question ?? (s.state === 'working' ? s.label : null) ?? null;
  const acoes = () => (
    <View style={styles.acoes}>
      {s.cwd ? <Pressable onPress={onGit} style={[styles.acao, { backgroundColor: theme.tokens.accent.base }]} accessibilityLabel="Git"><Icon name="GitBranch" size={18} color="#fff" /></Pressable> : null}
      <Pressable onPress={onExcluir} style={[styles.acao, { backgroundColor: theme.tokens.status.error }]} accessibilityLabel={m.sessao_excluir()}><Icon name="Trash2" size={18} color="#fff" /></Pressable>
    </View>
  );
  return (
    <ReanimatedSwipeable renderRightActions={acoes} rightThreshold={40} overshootRight={false}>
      <Pressable
        onPress={onPress}
        onLongPress={() => { Haptics.impactAsync(Haptics.ImpactFeedbackStyle.Medium); onLongPress(); }}
        delayLongPress={500}
        disabled={untracked && s.provider !== 'kimi'}
        style={({ pressed }) => [styles.row, pressed && { backgroundColor: theme.tokens.bg.hover }]}
        accessibilityRole="button"
        accessibilityLabel={s.name}
      >
        <View style={styles.lead}><StateDot state={s.state} /></View>
        <View style={styles.col}>
          <View style={styles.linha1}>
            <Text style={[styles.nome, { color: theme.tokens.text.primary }]} numberOfLines={1}>{s.name}</Text>
            {providerTag(s.provider) ? <Chip>{providerTag(s.provider)!}</Chip> : null}
            {untracked ? <Chip tone="warning">{m.sessao_sem_id()}</Chip> : null}
          </View>
          {sub ? <Text style={[styles.sub, { color: s.question ? theme.tokens.status.warning : theme.tokens.text.secondary, fontStyle: s.question ? 'normal' : 'italic' }]} numberOfLines={1}>{sub}</Text> : null}
          <View style={styles.meta}>
            {mostrarServidor ? <Text style={[styles.metaTxt, { color: s.serverColor }]}>{s.serverLabel}</Text> : null}
            {s.worktree ? <Chip mono>worktree</Chip> : null}
            {s.branch ? <Text style={[styles.metaTxt, styles.mono, { color: theme.tokens.accent.base }]} numberOfLines={1}>⎇ {s.branch}</Text> : null}
            {s.git_added || s.git_removed ? <Text style={[styles.metaTxt, styles.mono]}>{s.git_added ? <Text style={{ color: theme.tokens.status.success }}>+{s.git_added}</Text> : null}{s.git_removed ? <Text style={{ color: theme.tokens.status.error }}> −{s.git_removed}</Text> : null}</Text> : null}
            {showCwd ? <Text style={[styles.metaTxt, styles.mono, { color: theme.tokens.text.secondary, flexShrink: 1 }]} numberOfLines={1}>{cwd.prefix}{cwd.base}</Text> : null}
            <Text style={[styles.metaTxt, { color: theme.tokens.text.muted, marginLeft: 'auto' }]}>{relativeTime(s.last_activity)}</Text>
          </View>
          {(s.pair_peers?.length || s.limited || loop || plan || s.engine) ? (
            <View style={styles.chips}>
              {s.pair_peers?.length ? <Chip icon="Users">{s.pair_peers.length === 1 ? s.pair_peers[0] : String(s.pair_peers.length + 1)}</Chip> : null}
              {s.limited ? <Chip tone="warning" icon="Hourglass">{s.limit_reset ?? ''}</Chip> : null}
              {loop ? <Chip tone={({ ok: 'success', warn: 'warning', attention: 'error', muted: 'neutral' } as const)[loop.tone]}>{loop.label}</Chip> : null}
              {plan ? <Chip tone={plan.complete ? 'success' : 'accent'} icon="ClipboardList">{plan.label}</Chip> : null}
              {s.engine ? <Chip icon="Cog">{s.engine}</Chip> : null}
            </View>
          ) : null}
          {plan ? <PlanBar session={s} compact /> : null}
          {untracked && s.provider !== 'kimi' && s.provider !== 'pi' && s.provider !== 'omp'
            ? <Pressable onPress={onResume} style={styles.resume}><Text style={{ color: theme.tokens.accent.base, fontSize: theme.base.text.xs }}>↻ {m.sessao_retomar()}</Text></Pressable>
            : untracked ? <Text style={[styles.metaTxt, { color: theme.tokens.text.muted }]}>{untrackedReason(s.provider)}</Text> : null}
        </View>
        <Icon name="ChevronRight" size={16} color={theme.tokens.text.muted} />
      </Pressable>
    </ReanimatedSwipeable>
  );
}

const styles = StyleSheet.create((theme) => ({
  row: { flexDirection: 'row', alignItems: 'center', gap: 10, paddingVertical: 10, paddingHorizontal: 12, minHeight: 56 },
  lead: { width: 12, alignItems: 'center' },
  col: { flex: 1, gap: 3, minWidth: 0 },
  linha1: { flexDirection: 'row', alignItems: 'center', gap: 6 },
  nome: { fontSize: theme.base.text.base, fontWeight: '600', flexShrink: 1 },
  sub: { fontSize: theme.base.text.xs },
  meta: { flexDirection: 'row', alignItems: 'center', gap: 6, flexWrap: 'nowrap' },
  metaTxt: { fontSize: theme.base.text.xxs },
  mono: { fontFamily: theme.base.fontMono },
  chips: { flexDirection: 'row', flexWrap: 'wrap', gap: 4, marginTop: 2 },
  resume: { marginTop: 2 },
  acoes: { flexDirection: 'row' },
  acao: { width: 64, justifyContent: 'center', alignItems: 'center' },
}));
```

Os tons do `LoopBadge` são `ok | warn | attention | muted` (`packages/core/src/loop.ts:5`). Chave `sessao_excluir` já existe? `grep '"sessao_excluir"' messages/pt.json`; se não, criar ("Excluir"/"Delete").

- [x] **Step 6: `SessionMenu` (menu nativo no toque longo)**

`mobile/src/features/sessions/SessionMenu.tsx`:

```tsx
import { MenuView } from '@react-native-menu/menu';
import type { ReactNode } from 'react';
import * as m from '../../paraglide/messages';

export function SessionMenu({ children, temCwd, onRenomear, onGit, onLoop, onExcluir }: { children: ReactNode; temCwd: boolean; onRenomear: () => void; onGit: () => void; onLoop: () => void; onExcluir: () => void }) {
  return (
    <MenuView
      shouldOpenOnLongPress
      onPressAction={({ nativeEvent }) => ({ renomear: onRenomear, git: onGit, loop: onLoop, excluir: onExcluir })[nativeEvent.event]?.()}
      actions={[
        { id: 'renomear', title: m.sessao_renomear() },
        ...(temCwd ? [{ id: 'git', title: 'Git' }] : []),
        { id: 'loop', title: m.loop_titulo() },
        { id: 'excluir', title: m.sessao_excluir(), attributes: { destructive: true } },
      ]}
    >
      {children}
    </MenuView>
  );
}
```

**Primeiro build com isso é a prova do risco da spec**: se o app quebrar no `MenuView` em RN 0.86 (`@react-native-menu/menu` #1221/#1201), substituir o arquivo por `ContextMenu` de `@expo/ui/swift-ui` (iOS) e `@expo/ui/jetpack-compose` (Android) mantendo a mesma prop-interface, e registrar a troca no commit.

**Disputa de gesto** (segunda prova deste Step): o `MenuView` com `shouldOpenOnLongPress` e o `ReanimatedSwipeable` competem pelo mesmo toque. Ordem de aninhamento: `ReanimatedSwipeable` por fora, `SessionMenu` por dentro envolvendo só o `Pressable` da linha, e o `Pressable` **sem** `onLongPress` (o menu nativo é quem responde ao toque longo; o haptic vai no `onPressAction`). No emulador: segurar 600ms → menu abre e a linha não desliza; arrastar 40px pra esquerda → desliza e o menu não abre; toque curto → abre a sessão. Se no Android o swipe engolir o toque longo, passar `simultaneousHandlers` do `ReanimatedSwipeable` apontando pra um `Gesture.LongPress()` que chama `menuRef.current?.show()` em vez de `shouldOpenOnLongPress` — decisão registrada no commit, não contornada.

- [x] **Step 7: `AttentionStrip`, `ServerSheet`, filtro e a `SectionList`**

`AttentionStrip.tsx`: faixa horizontal no topo com `attentionFeed(rows)` — um `Chip tone="warning" icon="CircleHelp"` por sessão aguardando (`nome · pergunta` truncada), toque abre a sessão; some quando vazio.
`ServerSheet.tsx`: `Sheet sizes={['auto']}` com os servidores de `useServers`, ativo marcado, toque seleciona (`useServers.getState().setActive(id)`, `stores/servers.ts:24`), botão "Adicionar" → `router.push('/login')`.

`SessionList.tsx` reescrita:
- estados: `filtro` (string), `agrupar` de `useAparencia`; `const modo = effectiveGroupBy(agrupar, servers.length)`; `const grupos = agruparSessoes(rows.filter((r) => !filtro || r.name.toLowerCase().includes(filtro.toLowerCase()) || (r.cwd ?? '').toLowerCase().includes(filtro.toLowerCase())), modo)`;
- `SectionList sections={grupos.map((g) => ({ ...g, data: g.sessions }))}` `stickySectionHeadersEnabled` `renderSectionHeader={({ section }) => modo === 'none' ? null : <Glass variant="chrome" style={styles.header}><Text style={{ color: section.color ?? theme.tokens.text.secondary }}>{section.label}</Text></Glass>}` `renderItem={({ item }) => <SessionMenu …><SessionRow session={item} mostrarServidor={modo !== 'server' && servers.length > 1} … /></SessionMenu>}` `ItemSeparatorComponent` com linha de 1px em `border.subtle`; `ListHeaderComponent` com `AttentionStrip` e o campo de filtro (`MultilineInput` de 1 linha com `Icon name="Search"`); `refreshControl` como hoje;
- ações: `onGit` → `router.push(`/s/${id}/${name}/files`)` (a aba Git do celular é a de arquivos/diff que já existe), `onExcluir` → `Alert.alert(m.sessao_excluir(), name, [{ text: m.comum_cancelar(), style: 'cancel' }, { text: m.sessao_excluir(), style: 'destructive', onPress: () => deleteSession(name).then(() => toast.ok(...)).catch((e) => toast.erro(formataErro(e))) }])`, `onRenomear` → `Alert.prompt` no iOS e uma `Sheet` com `MultilineInput` no Android, `onLoop` → rota `loop`, `onResume` → `resumeSession(name)` com `toast.erro` na falha;
- apagar o bloco `demo-aguardando`. Pra manter a prova visual dos 3 estados sem linha sintética em produção, mover a fabricação pra um fixture de teste: `mobile/src/features/sessions/fixtures.ts` exporta `sessoesDemo(): AggSession[]` (idle, working, awaiting_input) e `agruparSessoes.test.ts` do core já cobre o agrupamento; a lista NÃO importa o fixture. Teste `demo.test.ts`: renderiza a lógica pura da lista — `agruparSessoes(rows, 'server')` com `rows` vindas do store mockado sem nenhuma `awaiting_input` — e garante que nenhum grupo contém sessão que não veio de `rows` (`expect(g.flatMap((x) => x.sessions)).toHaveLength(rows.length)`).
- Apagar `SessionCard.tsx`; `app/index.tsx` ganha, ao lado do `+`, um botão `Icon name="Server"` que abre a `ServerSheet` e um `Icon name="Settings"` (rota `/config`, criada na Task 5; até lá o botão fica sem `onPress`).

Chaves: `lista_filtro_placeholder` ("Filtrar sessões…"/"Filter sessions…"), `lista_agrupar_servidor`/`_projeto`/`_nao` ("Por servidor"/"Por projeto"/"Sem agrupar" · "By server"/"By project"/"No grouping") — usadas no menu de agrupamento (um `MenuView` no cabeçalho da lista) e na tela Geral da Task 5.

- [x] **Step 8: Typecheck, testes, prova visual**

Run: `npm run check -w @hangar/core && npm run typecheck -w mobile`; `npm run test -w @hangar/core && npm run test -w mobile`.
Expected: 0 erros; core +3; mobile +2 (aparencia, demo).

Emulador: lista agrupada por servidor com cabeçalho fixo; linha densa com branch, chips de plano/loop; swipe pra esquerda mostra Git/Excluir; toque longo abre o menu nativo; filtro reduz a lista; faixa "precisa de você" quando houver sessão aguardando (criar uma sessão de teste e mandar uma pergunta se não houver). Prints `t3-lista.png`, `t3-swipe.png`, `t3-menu.png`. Comparação cega com `pwa-01-lista.png` regravado na hora (até 2 rodadas).

- [x] **Step 9: Commit**

```bash
git add packages/core/src/agruparSessoes.ts packages/core/src/agruparSessoes.test.ts packages/core/src/index.ts mobile/src/features/sessions mobile/src/stores/aparencia.ts mobile/src/stores/aparencia.test.ts mobile/app/index.tsx messages/pt.json messages/en.json
git commit -m "feat(mobile): lista densa de sessões em seções com cabeçalho fixo, chips, swipe e menu nativo; filtro e faixa de atenção"
```

---

### Task 4: Material — papel de parede, transparência, solidez, acento

**Files:**
- Create: `packages/core/src/corTema.ts` (derivação pura movida de `frontend/src/lib/corTema.ts`), `packages/core/src/corTema.test.ts`
- Modify: `packages/core/src/theme.ts` (tipo `ThemeTokens` ganha nada; `theme.ts` exporta `comAcento(tokens, hex)`), `packages/core/src/index.ts`, `frontend/src/lib/corTema.ts` (usa a derivação do core)
- Create: `mobile/src/ui/Background.tsx`, `mobile/src/theme/aplicarMaterial.ts`, `mobile/src/theme/aplicarMaterial.test.ts`
- Modify: `mobile/src/stores/aparencia.ts`, `mobile/src/theme/unistyles.ts`, `mobile/app/_layout.tsx`, `mobile/src/ui/Glass.tsx`, `mobile/src/ui/Screen.tsx`

**Interfaces:**
- Produces (core): `comAcento(tokens: ThemeTokens, hex: string | null): ThemeTokens` (devolve tokens com `accent.base/dim/press` e `pill.working` derivados do hex; `null` = tokens originais).
- Produces (mobile): `useAparencia` ganha `fundo: 'flat'|'texture'|'aurora'|'image'`, `imagemUri: string|null`, `panelAlpha: number (0.3–1)`, `surfaceAlpha: number (0–1)`, `acento: string|null` e setters; `aplicarMaterial(state)` escreve nos dois temas via `UnistylesRuntime.updateTheme`; `Background` desenha o fundo escolhido atrás de tudo.

- [x] **Step 1: Teste da derivação de acento (core)**

Ler `frontend/src/lib/corTema.ts:11-60` para a fórmula de `dim`/`press` (mistura com alpha e escurecimento). `packages/core/src/corTema.test.ts`:

```ts
import { describe, it, expect } from 'vitest';
import { comAcento } from './corTema';
import { dark } from './theme';

describe('comAcento', () => {
  it('null devolve os tokens originais', () => { expect(comAcento(dark, null)).toBe(dark); });
  it('hex troca base, dim (18% alpha) e press (mais escuro)', () => {
    const t = comAcento(dark, '#ff0000');
    expect(t.accent.base).toBe('#ff0000');
    expect(t.accent.dim).toBe('rgba(255,0,0,0.18)');
    expect(t.accent.press).toMatch(/^#[0-9a-f]{6}$/);
    expect(t.accent.press).not.toBe('#ff0000');
    expect(t.pill.working.fg).not.toBe(dark.pill.working.fg);
  });
  it('hex inválido é ignorado', () => { expect(comAcento(dark, 'azul')).toBe(dark); });
});
```

- [x] **Step 2: Rodar e ver falhar** — `npx vitest run src/corTema.test.ts --root packages/core` → FAIL.

- [x] **Step 3: Implementar `packages/core/src/corTema.ts`**

```ts
import type { ThemeTokens } from './theme';

const HEX = /^#([0-9a-f]{6})$/i;
export function hexParaRgb(hex: string): [number, number, number] | null {
  const m = HEX.exec(hex);
  if (!m) return null;
  const n = parseInt(m[1], 16);
  return [(n >> 16) & 255, (n >> 8) & 255, n & 255];
}
const clamp = (v: number) => Math.max(0, Math.min(255, Math.round(v)));
const rgbParaHex = ([r, g, b]: [number, number, number]) => '#' + [r, g, b].map((c) => clamp(c).toString(16).padStart(2, '0')).join('');

// Mesma derivação do corTema.ts da PWA: dim = 18% do acento, press = acento 10% mais escuro,
// pill.working = acento a 16% de fundo e o texto clareado 20%.
export function comAcento(tokens: ThemeTokens, hex: string | null): ThemeTokens {
  const rgb = hex ? hexParaRgb(hex) : null;
  if (!rgb) return tokens;
  const [r, g, b] = rgb;
  return {
    ...tokens,
    accent: { base: rgbParaHex(rgb), dim: `rgba(${r},${g},${b},0.18)`, press: rgbParaHex([r * 0.9, g * 0.9, b * 0.9]) },
    pill: { ...tokens.pill, working: { bg: `rgba(${r},${g},${b},0.16)`, fg: rgbParaHex([r + (255 - r) * 0.2, g + (255 - g) * 0.2, b + (255 - b) * 0.2]) } },
  };
}
```

`index.ts`: `export * from './corTema';`. Em `frontend/src/lib/corTema.ts`, se houver função equivalente de derivação, trocar pelo `comAcento`/`hexParaRgb` do core (sem mudar comportamento; `npm run check -w frontend` e o `corTema.test.ts` da PWA continuam verdes). Run core → PASS.

- [x] **Step 4: Store de aparência: fundo, alphas, acento**

Em `mobile/src/stores/aparencia.ts`:

```ts
export type Fundo = 'flat' | 'texture' | 'aurora' | 'image';
// ...no estado:
fundo: (prefs.getString('aparencia.fundo') as Fundo) ?? 'flat',
imagemUri: prefs.getString('aparencia.imagemUri') ?? null,
panelAlpha: prefs.getNumber('aparencia.panelAlpha') ?? 0.86,
surfaceAlpha: prefs.getNumber('aparencia.surfaceAlpha') ?? 1,
acento: prefs.getString('aparencia.acento') ?? null,
setFundo, setImagemUri, setPanelAlpha, setSurfaceAlpha, setAcento  // cada um: prefs.set/remove + set() + aplicarMaterial(get())
```

`mobile/src/theme/aplicarMaterial.test.ts`:

```ts
import { describe, it, expect, vi } from 'vitest';
const updateTheme = vi.fn();
vi.mock('react-native-unistyles', async (orig) => ({ ...(await orig<object>()), UnistylesRuntime: { updateTheme } }));
import { aplicarMaterial } from './aplicarMaterial';

describe('aplicarMaterial', () => {
  it('escreve panelAlpha, surfaceAlpha e acento nos dois temas', () => {
    aplicarMaterial({ panelAlpha: 0.5, surfaceAlpha: 0.7, acento: '#ff0000' });
    expect(updateTheme).toHaveBeenCalledTimes(2);
    const [nome, fn] = updateTheme.mock.calls[0];
    expect(['light', 'dark']).toContain(nome);
    const novo = fn({ tokens: { accent: { base: '#000' }, pill: { working: {} } }, panelAlpha: 1, surfaceAlpha: 1 });
    expect(novo.panelAlpha).toBe(0.5);
    expect(novo.surfaceAlpha).toBe(0.7);
    expect(novo.tokens.accent.base).toBe('#ff0000');
  });
});
```

`mobile/src/theme/aplicarMaterial.ts`:

```ts
import { UnistylesRuntime } from 'react-native-unistyles';
import { comAcento, themeDark, themeLight } from '@hangar/core';

export function aplicarMaterial(s: { panelAlpha: number; surfaceAlpha: number; acento: string | null }) {
  for (const [nome, base] of [['light', themeLight], ['dark', themeDark]] as const) {
    UnistylesRuntime.updateTheme(nome, (t) => ({ ...t, tokens: comAcento(base, s.acento), panelAlpha: s.panelAlpha, surfaceAlpha: s.surfaceAlpha }));
  }
}
```

Run: `cd mobile && npx vitest run src/theme/aplicarMaterial.test.ts` → PASS. Em `_layout.tsx`, após `aplicarTemaSalvo()`: `aplicarMaterial(useAparencia.getState())`.

`Glass.tsx`: o `variant === 'panel'` já lê `theme.panelAlpha` — nada a mudar; no iOS 26, passar `tintColor` com esse alpha (já faz). Onde hoje há `backgroundColor: theme.tokens.bg.elevated` em componentes de conteúdo (`grep -rn "bg.elevated\|bg.base" mobile/src --include=*.tsx | grep backgroundColor`), trocar por `rgba(${glass.rgb},${surfaceAlpha})` como `Chip` e `ToolCard` fazem; deixar `bg.hover` só em `pressed`.

- [x] **Step 5: `Background`**

`mobile/src/ui/Background.tsx`:

```tsx
import { StyleSheet as RN, View } from 'react-native';
import { Image } from 'expo-image';
import { LinearGradient } from 'expo-linear-gradient';
import { useUnistyles } from 'react-native-unistyles';
import { useAparencia } from '../stores/aparencia';

// Atrás de tudo. flat = cor do tema; texture = gradiente sutil; aurora = três gradientes do acento;
// image = foto do aparelho (copiada pro documentDirectory no setImagemUri).
export function Background() {
  const { theme } = useUnistyles();
  const fundo = useAparencia((s) => s.fundo);
  const uri = useAparencia((s) => s.imagemUri);
  const base = theme.tokens.bg.base;
  const a = theme.tokens.accent.base;
  return (
    <View style={RN.absoluteFill} pointerEvents="none">
      <View style={[RN.absoluteFill, { backgroundColor: base }]} />
      {fundo === 'texture' ? <LinearGradient colors={[base, theme.tokens.bg.surface, base]} style={RN.absoluteFill} /> : null}
      {fundo === 'aurora' ? (<>
        <LinearGradient colors={[a + '66', 'transparent']} start={{ x: 0, y: 0 }} end={{ x: 1, y: 1 }} style={RN.absoluteFill} />
        <LinearGradient colors={['transparent', theme.tokens.chart[2] + '44']} start={{ x: 1, y: 0 }} end={{ x: 0, y: 1 }} style={RN.absoluteFill} />
      </>) : null}
      {fundo === 'image' && uri ? <Image source={{ uri }} style={RN.absoluteFill} contentFit="cover" transition={200} /> : null}
    </View>
  );
}
```

`mobile/src/ui/Screen.tsx`: renderizar `<Background />` como primeiro filho e o fundo próprio do `Screen` vira `transparent`. `setImagemUri` no store recebe o `uri` do `expo-image-picker` e copia com `FileSystem.copyAsync` pra `${FileSystem.documentDirectory}wallpaper.jpg` (import `* as FileSystem from 'expo-file-system/legacy'` — o SDK 57 tem a API nova; se `legacy` não existir, usar `new File(uri).copy(new File(Paths.document, 'wallpaper.jpg'))` de `expo-file-system`); falha na cópia → `toast.erro` e mantém o fundo anterior.

- [x] **Step 6: Typecheck, testes, prova visual**

Run: `npm run check -w @hangar/core && npm run typecheck -w mobile && npm run check -w frontend`; `npm run test -w @hangar/core && npm run test -w mobile`.
Expected: 0 erros; core +3; mobile +1 (aplicarMaterial) mais os do store.

Emulador (sem tela de configuração ainda): forçar pela `adb shell` não dá; então um teste manual temporário — em `_layout.tsx` chamar `useAparencia.getState().setFundo('aurora')` e `setPanelAlpha(0.5)` uma vez, tirar os prints `t4-aurora.png` (lista) e `t4-chat.png`, conferir que header/composer deixam o gradiente atravessar e que os cards ficam mais opacos que o painel; remover a chamada antes do commit. Regra do `CLAUDE.md`: retângulo que não deixa o fundo atravessar enquanto o painel em volta deixa é bug. Comparação com a PWA com aurora ligada (Aparência → Fundo), julgando opacidade e leiaute.

- [x] **Step 7: Commit**

```bash
git add packages/core/src/corTema.ts packages/core/src/corTema.test.ts packages/core/src/index.ts frontend/src/lib/corTema.ts mobile/src/ui/Background.tsx mobile/src/ui/Screen.tsx mobile/src/ui/Glass.tsx mobile/src/theme mobile/src/stores/aparencia.ts mobile/src/stores/aparencia.test.ts mobile/app/_layout.tsx
git commit -m "feat(mobile): material — papel de parede (flat/textura/aurora/imagem), transparência do painel, solidez das caixas e cor de acento via unistyles"
```

---

### Task 5: Configurações

**Files:**
- Create: `mobile/app/config/_layout.tsx`, `mobile/app/config/index.tsx`, `mobile/app/config/geral.tsx`, `mobile/app/config/aparencia.tsx`, `mobile/app/config/maquinas.tsx`, `mobile/app/config/sobre.tsx`
- Create: `mobile/src/features/config/Linha.tsx`, `mobile/src/features/config/Segmentado.tsx`, `mobile/src/features/config/Slider.tsx`
- Modify: `mobile/app/_layout.tsx` (rota `config` como `formSheet` grande), `mobile/app/index.tsx` (engrenagem abre `/config`), `mobile/src/stores/aparencia.ts` (`idioma: 'system'|'pt'|'en'`), `mobile/src/net/configureCore.ts` (respeita o override de idioma)
- Modify: `messages/pt.json`, `messages/en.json` (reusar as que existem: `config_geral_titulo`, `config_idioma_sistema|pt|en`, `config_idioma_nota_reload`, `config_tema_claro|escuro`, `config_modal_aparencia` (título "Aparência"), `maquinas_titulo`; novas: `config_sobre_titulo`, `config_sobre_versao`, `config_sobre_servidor`, `config_transparencia`, `config_solidez`, `config_acento`, `config_fundo_flat|texture|aurora|image`, `config_pensamento_tools`)

**Interfaces:**
- Consumes: `useAparencia` (tema, idioma, fundo, imagemUri, panelAlpha, surfaceAlpha, acento, pensamentoTools, agrupar), `useServers` (servers, active, add, remove, select), `getConfig()` do core (versão do servidor, se o `ConfigServidor` tiver o campo; senão só o `baseUrl`), `Constants.expoConfig?.version` de `expo-constants`.
- Produces: `Linha({ titulo, descricao?, children | onPress, icon? })`, `Segmentado({ opcoes: {v,label}[], valor, onChange })`, `Slider({ valor, min, max, onChange })` (sobre `@react-native-community/slider`).

- [x] **Step 1: Átomos de configuração**

`Linha.tsx`: linha com ícone opcional (`Icon`), título, descrição pequena, e à direita `children` ou chevron quando `onPress`. `Segmentado.tsx`: `View` com `Pressable` por opção, a selecionada com `backgroundColor: theme.tokens.accent.dim` e texto em `accent.base`. `Slider.tsx`: `import Slider from '@react-native-community/slider'` com `minimumTrackTintColor={theme.tokens.accent.base}` e `onSlidingComplete` → `onChange` (não a cada pixel: `updateTheme` a cada movimento é caro; o valor visual intermediário fica só no slider).

- [x] **Step 2: Rotas**

`app/config/_layout.tsx`: `<Stack screenOptions={{ headerShown: true, headerTransparent: true, headerBlurEffect: 'regular' }} />` com títulos vindos de `m.*`. `app/_layout.tsx`: `<Stack.Screen name="config" options={{ presentation: 'formSheet', sheetAllowedDetents: [0.92], sheetGrabberVisible: true, headerShown: false }} />`.

`app/config/index.tsx`: `Screen` com uma `Linha` por tela (Geral `Settings2`, Aparência `Palette`, Máquinas `Server`, Sobre `Info`) → `router.push('/config/geral')` etc.

`app/config/geral.tsx`: `Linha` Idioma → `Segmentado` (sistema/pt/en, `useAparencia.idioma`); `Linha` Tema → `Segmentado` (sistema/claro/escuro); `Linha` "O que entra no pensamento" → `Segmentado` (nada/busca/tudo); `Linha` "Agrupar sessões" → `Segmentado` (servidor/projeto/não). Idioma: `setIdioma` grava em MMKV e `configureCore.ts` lê `useAparencia.getState().idioma` antes de `getLocales()` (override manual vence o sistema); troca exige recarregar as mensagens → `toast.ok(m.config_idioma_nota_reload())` e `DevSettings.reload()` só em dev; em produção o texto avisa que vale na próxima abertura.

`app/config/aparencia.tsx`: `Segmentado` de fundo (flat/textura/aurora/imagem); botão "Escolher imagem" (`ImagePicker.launchImageLibraryAsync({ mediaTypes: ['images'], quality: 0.8 })` → `setImagemUri`) visível quando `image`; `Slider` Transparência (0.3–1 → `setPanelAlpha`); `Slider` Solidez (0–1 → `setSurfaceAlpha`); acento: fileira de 8 círculos com as cores de `frontend/src/lib/corTema.ts` (copiar a paleta literal) + "Padrão" (`setAcento(null)`).

`app/config/maquinas.tsx`: lista de `useServers.servers` com `StateDot` (ativo em `success`), label, host; toque seleciona; swipe/long-press remove (com `Alert`); botão "Adicionar" → `/login`. Testar conexão: `getConfigForServer(s)` com `toast.ok`/`toast.erro`.

`app/config/sobre.tsx`: `Constants.expoConfig?.version`, `Constants.expoConfig?.runtimeVersion`, servidor ativo (`baseUrl`) e `getConfig().somente_leitura` (é um `Record<string, string|number|boolean>`; mostrar as chaves `versao`/`version`/`commit` se vierem, senão só o endereço), link `https://github.com/jeffer1312/hangar` via `Linking.openURL`.

`app/index.tsx`: a engrenagem da Task 3 ganha `onPress={() => router.push('/config')}`.

- [x] **Step 3: Typecheck, testes, prova visual**

Run: `npm run typecheck -w mobile && npm run test -w mobile`.
Expected: 0 erros; tudo verde.

Emulador: engrenagem → `/config`; cada tela abre; trocar tema pra claro muda a lista sem reabrir; slider de transparência muda o header ao soltar; escolher aurora muda o fundo; idioma pt/en com aviso de recarga; Máquinas lista o servidor local e testa conexão; Sobre mostra a versão. Prints `t5-config.png`, `t5-aparencia.png`, `t5-maquinas.png`.

- [x] **Step 4: Commit**

```bash
git add mobile/app/config mobile/app/_layout.tsx mobile/app/index.tsx mobile/src/features/config mobile/src/stores/aparencia.ts mobile/src/stores/aparencia.test.ts mobile/src/net/configureCore.ts messages/pt.json messages/en.json
git commit -m "feat(mobile): configurações — Geral, Aparência, Máquinas e Sobre"
```

---

## Fechamento

- [x] **Step 1: Suítes completas, uma vez**

`npm run check` (raiz: core + frontend) e `npm run typecheck -w mobile`; `npm run test` (raiz) e `npm run test -w mobile`. Tudo verde.

- [x] **Step 2: Revisão da branch**

Uma sessão fresca revisa `git diff 0b5aba38..HEAD` contra a spec: cada item "Não tem" do inventário (`docs/pesquisa-aparencia-mobile-2026-09-05.md`, seção 3) agora tem arquivo apontado, ou está declarado fora de escopo. Rodar `ecc:typescript-reviewer`, `ecc:react-reviewer` e `ecc:silent-failure-hunter` no diff antes do push (o hook exige).

- [ ] **Step 3: Push da `mobile-expo`** (só com o usuário pedindo).
