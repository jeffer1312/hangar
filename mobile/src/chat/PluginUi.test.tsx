// @vitest-environment happy-dom
import { act, createElement, forwardRef, useImperativeHandle, useState, type ReactNode } from 'react';
import type { ChangeEvent, KeyboardEvent } from 'react';
import { createRoot } from 'react-dom/client';
import { afterEach, beforeEach, expect, test, vi } from 'vitest';
import { configureApi, parsePluginUi, type PluginNode } from '@hangar/core';
import { PluginTree, PluginUi } from './PluginUi';
import { chatStore, _resetChatsForTests } from '../stores/chat';
import { useMods } from '../stores/mods';
import { MoreSheet } from './MoreSheet';
import * as Clipboard from 'expo-clipboard';
import { Linking } from 'react-native';
import * as m from '../paraglide/messages';
import { toast } from '../ui/Toast';

(globalThis as typeof globalThis & { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
vi.mock('react-native', async importOriginal => {
  const original = await importOriginal<typeof import('react-native')>();
  return { ...original,
    Text: ({ numberOfLines, ellipsizeMode, ...props }: import('react-native').TextProps) => createElement(original.Text, { ...props, ...{ 'data-lines': numberOfLines, 'data-ellipsis': ellipsizeMode } }),
    TextInput: ({ value, editable, onChangeText, onFocus, onBlur, onSubmitEditing }: {
    value: string; editable?: boolean; onChangeText: (value: string) => void; onFocus: () => void; onBlur: () => void; onSubmitEditing: () => void;
  }) => createElement('input', { value, disabled: editable === false, onFocus, onBlur,
    onChange: (event: ChangeEvent<HTMLInputElement>) => onChangeText(event.target.value),
    onKeyDown: (event: KeyboardEvent) => { if (event.key === 'Enter') onSubmitEditing(); },
  }) };
});
vi.mock('react-native-mmkv', () => ({ createMMKV: () => ({ getString: () => undefined, set: () => {} }) }));
vi.mock('expo-clipboard', () => ({ setStringAsync: vi.fn(async () => {}) }));
const servers = vi.hoisted(() => [{ id: 'srv2', label: 'two', baseUrl: 'http://10.0.0.2:8765', token: 'two-token' }]);
vi.mock('../stores/servers', () => ({ useServers: Object.assign((select: (s: unknown) => unknown) => select({ servers }), { getState: () => ({ servers }) }) }));
const nativeSheet = vi.hoisted(() => ({ finishDismiss: null as (() => void) | null }));
vi.mock('@lodev09/react-native-true-sheet', () => ({ TrueSheet: forwardRef(function NativeSheet({ children, onDidDismiss }: { children: ReactNode; onDidDismiss?: () => void }, ref) {
  const [visible, setVisible] = useState(false);
  useImperativeHandle(ref, () => ({ present: async () => { setVisible(true); }, dismiss: async () => { setVisible(false); } }));
  nativeSheet.finishDismiss = () => { setVisible(false); onDidDismiss?.(); };
  return visible ? createElement('div', null, children) : null;
}) }));
vi.mock('expo-blur', async () => ({ BlurView: (await import('react-native')).View }));
vi.mock('expo-glass-effect', async () => ({ GlassView: (await import('react-native')).View, isLiquidGlassAvailable: () => false }));
vi.mock('expo-router', () => ({ useRouter: () => ({ push: vi.fn() }) }));
vi.mock('./AssistantBubble', () => ({ mkMarkdownStyle: () => ({}) }));
vi.mock('react-native-enriched-markdown', () => ({
  EnrichedMarkdownText: ({ markdown }: { markdown: string }) => createElement('pre', null, markdown),
}));
const keyboard = vi.hoisted(() => ({ height: 0 }));
vi.mock('react-native-keyboard-controller', () => ({ useKeyboardState: (select: (state: typeof keyboard) => unknown) => select(keyboard) }));
vi.mock('../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));

const roots: ReturnType<typeof createRoot>[] = [];
beforeEach(() => {
  keyboard.height = 0;
  nativeSheet.finishDismiss = null;
  useMods.setState({ visible: false });
  configureApi({ getBaseUrl: () => 'http://wrong-server', getToken: () => 'wrong-token', onUnauthorized: () => {}, origin: null,
    createEventSource: () => ({ addEventListener: () => {}, removeEventListener: () => {}, close: () => {}, onerror: null, onopen: null, readyState: 1 }),
  });
  vi.stubGlobal('fetch', vi.fn(async () => new Response(JSON.stringify({ ok: true }), { status: 200 })));
  vi.spyOn(Linking, 'openURL').mockResolvedValue(undefined);
  vi.clearAllMocks();
});
afterEach(() => {
  roots.splice(0).forEach(root => act(() => root.unmount()));
  _resetChatsForTests();
  vi.unstubAllGlobals();
});
function render(node: PluginNode, onPress?: (control: { plugin: string; key: string }) => void) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(createElement(PluginTree, { node, onPress })));
  return container;
}

test('árvore nativa preserva texto aninhado e endereça botão pelo mod e pela key', () => {
  const onPress = vi.fn();
  const container = render({ type: 'Box', children: [
    { type: 'Text', children: ['prefix-', { type: 'Text', props: { bold: true }, children: ['suffix'] }] },
    { type: 'Button', props: { key: 'run', label: 'run-mod' }, press: { plugin: 'mod-a' } },
    { type: 'unknown', children: ['unknown-child'] },
    { type: 'Image', props: { alt: 'image-alt' } },
  ] }, onPress);
  expect(container.textContent).toContain('prefix-suffix');
  expect(container.textContent).toContain('unknown-child');
  expect(container.textContent).toContain('image-alt');
  act(() => container.querySelector<HTMLButtonElement>('button')!.click());
  expect(onPress).toHaveBeenCalledExactlyOnceWith({ plugin: 'mod-a', key: 'run' });
});

test('botão sem destino fica inerte e conteúdo Markdown usa renderer', () => {
  const container = render({ type: 'Box', children: [
    { type: 'Button', props: { label: 'no-address' } },
    { type: 'Markdown', props: { text: '**markdown-mod**' } },
    { type: 'Code', props: { source: 'code-mod' } },
  ] }, vi.fn());
  expect(container.querySelector('button')).toBeNull();
  expect(container.querySelector('pre')?.textContent).toBe('**markdown-mod**');
  expect(container.textContent).toContain('code-mod');
});

const surface = {
  above: { type: 'Text', children: ['band-live'] },
  panes: [
    { id: 'one', title: 'pane-one', tree: { type: 'Text', children: ['pane-first'] } },
    { id: 'two', title: 'pane-two', tree: { type: 'Button', props: { key: 'run', label: 'run-mod' }, press: { plugin: 'mod-a' } } },
  ],
  source: 'surface', shown_id: 'two',
};
function renderUi(readOnly = false) {
  chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi(surface) });
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(createElement(PluginUi, { serverId: 'srv2', name: 'sess', readOnly })));
  return container;
}
const labeled = (container: HTMLElement, label: string) => container.querySelector<HTMLElement>(`[aria-label="${label}"]`)!;

test('mods começam ocultos, menu mostra e oculta sem fechar painel do agente', async () => {
  const container = renderUi();
  expect(container.textContent).toBe('');
  const menu = document.createElement('div');
  const root = createRoot(menu);
  roots.push(root);
  const renderMenu = (open: boolean): ReactNode => createElement(MoreSheet, { open,
    onClose: () => root.render(renderMenu(false)), serverId: 'srv2', name: 'sess' });
  await act(async () => root.render(renderMenu(true)));
  await act(async () => labeled(menu, m.mods_celular_mostrar_paineis({ n: '2' })).click());
  expect(container.textContent).toBe('');
  await act(async () => nativeSheet.finishDismiss!());
  expect(container.textContent).toContain('band-live');
  expect(container.textContent!.indexOf('run-mod')).toBeLessThan(container.textContent!.indexOf('band-live'));
  await act(async () => root.render(renderMenu(true)));
  await act(async () => labeled(menu, m.mods_celular_ocultar()).click());
  expect(container.textContent).toContain('band-live');
  await act(async () => nativeSheet.finishDismiss!());
  expect(container.textContent).toBe('');
  expect(fetch).not.toHaveBeenCalled();
});

test('ação de mod usa servidor da conversa, bloqueia repetição e aplica copiar e abrir no aparelho', async () => {
  let finish!: (value: Response) => void;
  vi.mocked(fetch).mockImplementationOnce(() => new Promise<Response>(resolve => { finish = resolve; }));
  useMods.setState({ visible: true });
  const container = renderUi();
  act(() => { labeled(container, 'run-mod').click(); labeled(container, 'run-mod').click(); });
  expect(fetch).toHaveBeenCalledTimes(1);
  const [url, request] = vi.mocked(fetch).mock.calls[0];
  expect(url).toBe('http://10.0.0.2:8765/api/sessions/sess/plugin/press');
  expect(JSON.parse(request!.body as string)).toEqual({ site: 'two', plugin: 'mod-a', key: 'run' });
  expect((request!.headers as Record<string, string>).Authorization).toBe('Bearer two-token');
  await act(async () => finish(new Response(JSON.stringify({ ok: true, copied: 'copy-mod', opened: 'https://example.test/mod' }), { status: 200 })));
  expect(Clipboard.setStringAsync).toHaveBeenCalledWith('copy-mod');
  expect(Linking.openURL).toHaveBeenCalledWith('https://example.test/mod');
});

test('recusa permanece visível e só leitura não envia clique, campo ou fechamento', async () => {
  useMods.setState({ visible: true });
  vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify({ detail: 'mod-refused' }), { status: 409 }));
  const container = renderUi();
  await act(async () => labeled(container, 'run-mod').click());
  expect(container.querySelector('[role="alert"]')?.textContent).toContain('mod-refused');
  expect(chatStore('srv2', 'sess').use.getState().pluginUi?.panes).toHaveLength(2);
  const readOnly = renderUi(true);
  expect(labeled(readOnly, m.plugin_painel_fechar())).toBeNull();
  expect(labeled(readOnly, 'run-mod')).toBeNull();
  expect(fetch).toHaveBeenCalledTimes(1);
});

test('delta sem base fica visível mesmo com mods ocultos; painel sem desenho não fica carregando', () => {
  const container = renderUi();
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUiError: m.plugin_ui_falhou() }));
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.plugin_ui_falhou());
  act(() => {
    useMods.setState({ visible: true });
    chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ panes: [{ id: 'loading', tree: { type: 'engine' } }] }), pluginUiError: null });
  });
  expect(container.textContent).toContain('loading');
  expect(container.textContent).not.toContain(m.plugin_painel_carregando());
  // O painel só de estado do `/btw` (árvore nula) que chegou de uma sessão anterior não gira para sempre.
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ above: null,
    panes: [{ id: 'hangar-btw', title: '', placement: 'inline', columns: null, tree: null }] }) }));
  expect(container.textContent).toContain('hangar-btw');
  expect(container.textContent).not.toContain(m.plugin_painel_carregando());
});

test('troca e fechamento esperam confirmação do servidor e campo do terminal fica bloqueado', async () => {
  useMods.setState({ visible: true });
  const container = renderUi();
  await act(async () => labeled(container, 'pane-one').click());
  expect(vi.mocked(fetch).mock.calls[0][0]).toBe('http://10.0.0.2:8765/api/sessions/sess/plugin/show');
  expect(container.textContent).not.toContain('pane-first');
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ ...surface, shown_id: 'one' }) }));
  expect(container.textContent).toContain('pane-first');
  await act(async () => labeled(container, m.plugin_painel_fechar()).click());
  expect(vi.mocked(fetch).mock.calls[1][0]).toBe('http://10.0.0.2:8765/api/sessions/sess/plugin/close');
  expect(container.textContent).toContain('pane-first');
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ above: { type: 'Input', props: { key: 'field' }, press: { plugin: 'mod-a' } }, panes: [], source: 'terminal' }) }));
  expect(container.querySelector<HTMLInputElement>('input')?.disabled).toBe(true);
  expect(fetch).toHaveBeenCalledTimes(2);
});

test('só leitura troca painéis localmente sem escrever no servidor', () => {
  useMods.setState({ visible: true });
  const container = renderUi(true);
  expect(container.textContent).toContain('run-mod');
  act(() => labeled(container, 'pane-one').click());
  expect(container.textContent).toContain('pane-first');
  expect(container.textContent).not.toContain('run-mod');
  expect(fetch).not.toHaveBeenCalled();
});

test('erro do stream bloqueia abas sem trocar o painel local nem chamar o servidor', () => {
  useMods.setState({ visible: true });
  const container = renderUi();
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ ...surface, shown_id: undefined }), pluginUiError: m.plugin_ui_falhou() }));
  const tab = labeled(container, 'pane-one') as HTMLButtonElement;
  expect(tab.disabled).toBe(true);
  act(() => tab.click());
  expect(container.textContent).toContain('run-mod');
  expect(container.textContent).not.toContain('pane-first');
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.plugin_ui_falhou());
  expect(fetch).not.toHaveBeenCalled();
});

test.each(['javascript:alert(1)', 'file:///tmp/mod'])('link inseguro na resposta de clique avisa e não abre: %s', async opened => {
  useMods.setState({ visible: true });
  vi.mocked(fetch).mockResolvedValueOnce(new Response(JSON.stringify({ ok: true, opened }), { status: 200 }));
  const container = renderUi();
  await act(async () => labeled(container, 'run-mod').click());
  expect(toast.erro).toHaveBeenCalledWith(m.plugin_link_inseguro());
  expect(Linking.openURL).not.toHaveBeenCalled();
});

test('sessão morta não oferece interface de mods guardada no menu', () => {
  renderUi();
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  act(() => root.render(createElement(MoreSheet, { open: true, onClose: () => {}, serverId: 'srv2', name: 'sess' })));
  expect(labeled(container, m.mods_celular_mostrar_paineis({ n: '2' }))).not.toBeNull();
  act(() => chatStore('srv2', 'sess').use.setState({ stateEvent: { session: 'sess', state: 'dead' } }));
  expect(labeled(container, m.mods_celular_mostrar_paineis({ n: '2' }))).toBeNull();
});

test('campo que perde edição preserva o texto local e avisa sem enviar mudanças pendentes', async () => {
  let finish!: () => void;
  const onInput = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  const frame: PluginNode = { type: 'Input', props: { key: 'field', value: '' }, press: { plugin: 'mod-a' } };
  act(() => root.render(createElement(PluginTree, { node: frame, onInput })));
  const input = container.querySelector<HTMLInputElement>('input')!;
  act(() => input.focus());
  const change = (value: string) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input.dispatchEvent(new Event('input', { bubbles: true }));
  };
  act(() => change('first'));
  act(() => change('local-pending'));
  act(() => root.render(createElement(PluginTree, { node: frame })));
  expect(input.disabled).toBe(true);
  expect(input.value).toBe('local-pending');
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.plugin_input_desativado());
  expect(container.querySelector('button')).toBeNull();
  act(() => input.blur());
  act(() => root.render(createElement(PluginTree, { node: { ...frame, props: { key: 'field', value: 'server-known' } } })));
  expect(input.value).toBe('local-pending');
  await act(async () => finish());
  expect(onInput).toHaveBeenCalledTimes(1);
  expect(container.querySelector('[role="alert"]')?.textContent).toBe(m.plugin_input_desativado());
  container.remove();
});

test('texto de um nó herda cor e ênfase, sem trocar pela cor padrão nos filhos', () => {
  const container = render({ type: 'Text', props: { color: 'red', bold: true }, children: ['colored'] });
  document.body.append(container);
  const leaf = [...container.querySelectorAll('span')].find(el => el.firstChild?.nodeType === 3)!;
  expect(getComputedStyle(leaf).color).toBe('#cd3131');
  container.remove();
});

const progressTree = (collapsed: boolean): PluginNode => {
  const width = collapsed ? 308 : 721;
  const bar: PluginNode = { type: 'Svg', props: { width, height: 6, source: `<svg width="${width}" height="6" viewBox="0 0 ${width} 6"><rect width="100%" height="6" /></svg>` } };
  return { type: 'Box', props: { flexDirection: 'column' }, children: [{ type: 'Box', props: { flexDirection: 'column' }, children: [
    { type: 'Box', props: { key: 'header', flexDirection: 'row', justifyContent: 'space-between', width: 110 }, children: [
      { type: 'Box', props: { flexDirection: 'row' }, children: [
        { type: 'Text', children: ['● '] },
        { type: 'Button', props: { key: 'toggle', label: `${collapsed ? '▸' : '▾'} title-mod`, plain: true }, press: { plugin: 'fixture-mod' } },
        { type: 'Text', props: { dimColor: true, wrap: 'truncate-end' }, children: [' (step-mod)'] },
      ] },
      collapsed ? { type: 'Box', props: { flexDirection: 'row' }, children: [{ type: 'Text', children: [' '] }, bar, { type: 'Text', children: [' 50% · 3m 13s'] }] }
        : { type: 'Text', children: [' 1/2 steps · 3m 30s'] },
    ] },
    ...(!collapsed ? [{ type: 'Box', props: { flexDirection: 'row' }, children: [{ type: 'Text', children: ['  '] }, bar, { type: 'Text', children: [' 50%'] }] }] : []),
  ] }] };
};

test.each([false, true])('árvore real do progresso mantém linha alinhada e SVG encolhível: recolhida=%s', collapsed => {
  useMods.setState({ visible: true });
  const container = renderUi();
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({ above: progressTree(collapsed), panes: [], columns: 110, source: 'surface' }) }));
  const title = labeled(container, `${collapsed ? '▸' : '▾'} title-mod`);
  expect(title.style.minHeight).toBe('');
  expect(Number(title.querySelector('span')!.style.lineHeight)).toBe(21);
  const header = title.parentElement!.parentElement!;
  expect(header.style.width).toBe('100%');
  expect(header.style.maxWidth).toBe('100%');
  const drawing = container.querySelector('svg')!.parentElement!;
  expect(drawing.style.width).toBe('100%');
  expect(drawing.style.flexShrink).toBe('1');
  const trailing = drawing.nextElementSibling as HTMLElement;
  expect(trailing.textContent?.trim()).toBe(collapsed ? '50% · 3m 13s' : '50%');
  expect(trailing.style.flexShrink).toBe('0');
  expect(drawing.style.maxWidth).toBe(collapsed ? '308px' : '721px');
  expect(drawing.style.alignSelf).toBe('center');
});

test.each(['CI', 'Server'])('grupo que cede largura corta Text e Button numa linha e preserva o lado fixo: %s', title => {
  useMods.setState({ visible: true });
  const container = renderUi();
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({
    columns: 110, source: 'surface', panes: [], above: { type: 'Box', props: { flexDirection: 'column' }, children: [
      { type: 'Box', props: { flexDirection: 'row', width: 108, justifyContent: 'space-between', columnGap: 1 }, children: [
        { type: 'Box', props: { flexDirection: 'row', flexShrink: 1, columnGap: 1 }, children: [
          { type: 'Text', children: ['✓'] },
          { type: 'Button', props: { key: 'toggle', label: `▸ ${title}`, plain: true }, press: { plugin: 'fixture-mod' } },
          { type: 'Text', props: { dimColor: true }, children: ['abc1234'] },
        ] },
        { type: 'Box', props: { flexDirection: 'row', flexShrink: 0, columnGap: 2 }, children: [
          { type: 'Text', children: ['7/7 jobs · 9m 28s'] }, { type: 'Text', children: ['100%'] },
          { type: 'Button', props: { key: 'open', label: `open-${title}` }, press: { plugin: 'fixture-mod' } },
        ] },
      ] },
    ] },
  }) }));
  const hash = [...container.querySelectorAll<HTMLElement>('span')].find(el => el.textContent === 'abc1234')!;
  expect(hash.dataset.lines).toBe('1');
  expect(hash.dataset.ellipsis).toBe('tail');
  expect(hash.style.flexShrink).toBe('1');
  expect(Number.parseFloat(hash.parentElement!.style.minWidth)).toBe(0);
  const button = labeled(container, `▸ ${title}`);
  expect(button.style.flexShrink).toBe('1');
  expect(button.querySelector<HTMLElement>('span')!.dataset.lines).toBe('1');
  const fixed = [...container.querySelectorAll<HTMLElement>('span')].find(el => el.textContent === '7/7 jobs · 9m 28s')!;
  expect(fixed.style.flexShrink).toBe('0');
  expect(labeled(container, `open-${title}`).style.flexShrink).toBe('0');
});

test('grupo fixo interrompe a herança de corte; texto primitivo no grupo flexível recebe o corte', () => {
  const container = render({ type: 'Box', props: { flexDirection: 'row', flexShrink: 1 }, children: [
    'flexible-primitive',
    { type: 'Box', props: { flexDirection: 'row', flexShrink: 0 }, children: [{ type: 'Text', children: ['fixed-label'] }] },
  ] });
  const primitive = [...container.querySelectorAll<HTMLElement>('span')].find(el => el.textContent === 'flexible-primitive')!;
  expect(primitive.dataset.lines).toBe('1');
  const fixed = [...container.querySelectorAll<HTMLElement>('span')].find(el => el.textContent === 'fixed-label')!;
  expect(fixed.style.flexShrink).toBe('0');
});

test.each([false, true])('muitas linhas usam região limitada com conteúdo natural: painel=%s', withPane => {
  useMods.setState({ visible: true });
  const container = renderUi();
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi({
    ...surface,
    above: { type: 'Box', props: { flexDirection: 'column' }, children: Array.from({ length: 8 }, (_, i) => ({ type: 'Text', children: [`mod-line-${i}`] })) },
    panes: withPane ? surface.panes : [],
  }) }));
  const band = labeled(container, m.plugin_band_label());
  expect(band.style.flexGrow).toBe('0');
  expect(band.style.flexShrink).toBe('1');
  expect(Number.parseFloat(band.style.maxHeight)).toBe(126.6);
  expect(band.textContent).toContain('mod-line-7');
  if (withPane) {
    const pane = labeled(container, m.plugin_painel_label({ titulo: 'pane-two' }));
    const body = pane.children[1] as HTMLElement;
    expect(body.style.flexGrow).toBe('0');
    expect(body.style.flexShrink).toBe('1');
  }
});

test('conjunto dos mods deixa espaço para o composer quando o teclado abre', () => {
  useMods.setState({ visible: true });
  const container = renderUi();
  keyboard.height = 344;
  act(() => chatStore('srv2', 'sess').use.setState({ pluginUi: parsePluginUi(surface) }));
  expect((container.firstElementChild as HTMLElement).style.maxHeight).toBe('200px');
});

test('inserção de texto antes do campo preserva identidade, foco e rascunho', async () => {
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  const onInput = vi.fn(async () => {});
  const inputNode: PluginNode = { type: 'Input', props: { key: 'stable', value: '' }, press: { plugin: 'mod-a' } };
  const frame = (children: PluginNode[]) => createElement(PluginTree, { node: { type: 'Box', children }, onInput });
  act(() => root.render(frame([inputNode])));
  const input = container.querySelector<HTMLInputElement>('input')!;
  act(() => input.focus());
  await act(async () => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, 'draft-mod');
    input.dispatchEvent(new Event('input', { bubbles: true }));
  });
  act(() => root.render(frame([{ type: 'Text', children: ['inserted'] }, { ...inputNode, props: { key: 'stable', value: '' } }])));
  expect(container.querySelector('input')).toBe(input);
  expect(input.value).toBe('draft-mod');
  expect(document.activeElement).toBe(input);
  container.remove();
});

test('campo serializa change e submit e não recebe eco antigo sobre a digitação', async () => {
  let finish!: () => void;
  const onInput = vi.fn(() => new Promise<void>(resolve => { finish = resolve; }));
  const container = document.createElement('div');
  document.body.append(container);
  const root = createRoot(container);
  roots.push(root);
  const field = (value: string) => createElement(PluginTree, { node: { type: 'Input', props: { key: 'field', label: 'field-label', value }, press: { plugin: 'mod-a' } }, onInput });
  act(() => root.render(field('')));
  const input = container.querySelector<HTMLInputElement>('input');
  expect(input).not.toBeNull();
  act(() => input!.focus());
  const change = (value: string) => {
    Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value')!.set!.call(input, value);
    input!.dispatchEvent(new Event('input', { bubbles: true }));
  };
  act(() => change('first'));
  act(() => change('latest'));
  act(() => {
    input!.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
    input!.dispatchEvent(new window.KeyboardEvent('keydown', { key: 'Enter', bubbles: true }));
  });
  expect(onInput).toHaveBeenCalledTimes(1);
  expect(onInput).toHaveBeenNthCalledWith(1, { plugin: 'mod-a', key: 'field' }, 'change', 'first');
  act(() => root.render(field('first')));
  expect(input!.value).toBe('latest');
  await act(async () => finish());
  expect(onInput).toHaveBeenNthCalledWith(2, { plugin: 'mod-a', key: 'field' }, 'change', 'latest');
  await act(async () => finish());
  expect(onInput).toHaveBeenNthCalledWith(3, { plugin: 'mod-a', key: 'field' }, 'submit', 'latest');
  await act(async () => finish());
  expect(onInput).toHaveBeenCalledTimes(3);
  act(() => root.render(field('')));
  expect(input!.value).toBe('');
  container.remove();
});
