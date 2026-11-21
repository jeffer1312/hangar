// @vitest-environment happy-dom
import { act, createElement, type ReactNode } from 'react';
import { createRoot, type Root } from 'react-dom/client';
import { afterEach, expect, it, vi } from 'vitest';
import type { GestureType } from 'react-native-gesture-handler';

(globalThis as { IS_REACT_ACT_ENVIRONMENT?: boolean }).IS_REACT_ACT_ENVIRONMENT = true;
const native = vi.hoisted(() => ({
  gestures: [] as GestureType[],
  style: () => ({ transform: [] as Record<string, number>[] }),
}));

// Construtores reais; só a entrega de eventos e a animação nativas são substituídas.
vi.mock('react-native-gesture-handler', async () => {
  const { GestureObjects } = await vi.importActual<{ GestureObjects: typeof import('react-native-gesture-handler').Gesture }>(
    new URL('../../../node_modules/react-native-gesture-handler/lib/module/handlers/gestures/gestureObjects.js', import.meta.url).pathname,
  );
  return {
    Gesture: GestureObjects,
    GestureHandlerRootView: ({ children }: { children: ReactNode }) => children,
    GestureDetector: ({ gesture, children }: { gesture: ReturnType<typeof GestureObjects.Simultaneous>; children: ReactNode }) => {
      gesture.prepare();
      native.gestures = gesture.toGestureArray();
      return children;
    },
  };
});
vi.mock('react-native', async (original) => ({ ...await original<object>(), StatusBar: { currentHeight: 24 } }));
// Só o estilo com `transform` é o da foto; os outros são o esmaecer do fundo e das barras.
vi.mock('react-native-reanimated', async (original) => ({
  ...await original<object>(),
  useAnimatedStyle: (fn: typeof native.style) => {
    if ('transform' in (fn() as object)) native.style = fn;
    return {};
  },
}));
vi.mock('./mediaCache', () => ({ localCopy: vi.fn(), saveImage: vi.fn(), shareFile: vi.fn(), canShareFile: true }));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../paraglide/messages', () => ({
  anexos_fechar_imagem: () => 'fechar', anexos_baixar: () => 'salvar',
  anexos_ver: ({ n }: { n: string }) => `abrir ${n}`, anexos_tocar: ({ nome }: { nome: string }) => `tocar ${nome}`,
  gallery_save: () => 'gallery_save', gallery_share: () => 'gallery_share', gallery_share_named: () => 'share',
  gallery_in_chat: () => 'in_chat', gallery_counter: ({ n, total }: { n: number; total: number }) => `${n}/${total}`,
}));

import { Lightbox, type LightboxProps } from './Lightbox';
import { ImageThumb } from '../../chat/ImageThumb';

const roots: Root[] = [];
afterEach(() => { act(() => roots.splice(0).forEach((root) => root.unmount())); });

async function render(props: Partial<LightboxProps> = {}) {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  const onClose = vi.fn();
  const onStep = vi.fn();
  await act(async () => root.render(createElement(Lightbox, {
    visible: true, uri: 'https://example.test/1.png', filename: '1.png', onClose, onStep, ...props,
  })));
  return { container, root, onClose, onStep };
}

function gesture(name: string) {
  const result = native.gestures.find((g) => g.handlerName === name);
  if (!result) throw new Error(`missing gesture: ${name}`);
  return result;
}

function tapGesture(taps: 1 | 2) {
  const result = native.gestures.find((g) => g.handlerName === 'TapGestureHandler'
    && ((g.config as { numberOfTaps?: number }).numberOfTaps ?? 1) === taps);
  if (!result) throw new Error(`missing tap: ${taps}`);
  return result;
}

function event(translationX = 0, translationY = 0, scale = 1, numberOfPointers = 1, velocityY = 0) {
  return { translationX, translationY, scale, numberOfPointers, velocityX: 0, velocityY, x: 0, y: 0 };
}

function deliver(name: string, phase: 'onBegin' | 'onStart' | 'onUpdate' | 'onEnd' | 'onFinalize', data = event(), success = true) {
  const callback = gesture(name).handlers[phase] as ((e: ReturnType<typeof event>, success: boolean) => void) | undefined;
  act(() => callback?.(data, success));
}

function tap(taps: 1 | 2, x = 0, y = 0) {
  const callback = tapGesture(taps).handlers.onEnd as ((e: ReturnType<typeof event>, success: boolean) => void) | undefined;
  act(() => callback?.({ ...event(), x, y }, true));
}

function drag(x: number, y = 0, success = true, velocityY = 0) {
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PanGestureHandler', 'onUpdate', event(x, y));
  deliver('PanGestureHandler', 'onEnd', event(x, y, 1, 1, velocityY), success);
  deliver('PanGestureHandler', 'onFinalize', event(x, y), success);
}

function translation() {
  return native.style().transform.find((t) => 'translateX' in t)?.translateX;
}
function zoom() {
  return native.style().transform.find((t) => 'scale' in t)?.scale;
}

it('without zoom the photo follows the finger and steps on release', async () => {
  const { onStep, onClose } = await render();
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PanGestureHandler', 'onUpdate', event(-95, 4));
  expect(translation()).toBe(-95);
  expect(onStep).not.toHaveBeenCalled();
  deliver('PanGestureHandler', 'onEnd', event(-95, 4));
  deliver('PanGestureHandler', 'onFinalize', event(-95, 4));
  expect(onStep).toHaveBeenCalledExactlyOnceWith(1);
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it('the close tap does not share the gallery drag on iOS, and the double tap comes first', async () => {
  await render();
  const pan = gesture('PanGestureHandler');
  const pinch = gesture('PinchGestureHandler');
  const single = tapGesture(1);
  const double = tapGesture(2);
  expect(pan.config.simultaneousWith).not.toContain(single);
  expect(pan.config.simultaneousWith).toContain(pinch);
  expect(single.config).toMatchObject({ maxDist: 10 });
  // Exclusive: o toque simples só vale quando o duplo falha.
  expect(single.config.requireToFail).toContain(double);
});

it('a cancelled drag goes back to the center without stepping', async () => {
  const { onStep, onClose } = await render();
  drag(-100, 0, false);
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it('a pinch back to the original size is not a step', async () => {
  const { onStep } = await render();
  deliver('PanGestureHandler', 'onBegin');
  deliver('PanGestureHandler', 'onStart');
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 1.5, 2));
  deliver('PanGestureHandler', 'onUpdate', event(100, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onEnd');
  deliver('PanGestureHandler', 'onEnd', event(100));
  deliver('PanGestureHandler', 'onFinalize', event(100));
  expect(onStep).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it.each([[25, 0], [100, -150], [10, 80]])('short, upward or short downward drag (%s, %s) neither steps nor closes', async (x, y) => {
  const { onStep, onClose } = await render();
  drag(x, y);
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
  expect(translation()).toBe(0);
});

it.each([[10, 200, 0], [5, 60, 1500]])('dragging down (%s, %s, v=%s) closes', async (x, y, v) => {
  const { onStep, onClose } = await render();
  drag(x, y, true, v);
  expect(onClose).toHaveBeenCalledTimes(1);
  expect(onStep).not.toHaveBeenCalled();
});

it('with zoom the pan accumulates, the tap does not close and dragging down does not close', async () => {
  const { onStep, onClose } = await render();
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 2, 2));
  deliver('PinchGestureHandler', 'onEnd');
  drag(100, 20);
  drag(40, 10);
  tap(1);
  expect(translation()).toBe(140);
  expect(native.style().transform).toContainEqual({ translateY: 30 });
  expect(zoom()).toBe(2);
  drag(0, 300);
  expect(onStep).not.toHaveBeenCalled();
  expect(onClose).not.toHaveBeenCalled();
});

it('double tap zooms in and a second one goes back', async () => {
  const { onClose } = await render();
  tap(2);
  expect(zoom()).toBe(2.5);
  tap(2);
  expect(zoom()).toBe(1);
  expect(translation()).toBe(0);
  expect(onClose).not.toHaveBeenCalled();
});

it('a single image does not slide and the single tap still closes', async () => {
  const { onClose } = await render({ onStep: undefined });
  drag(-100);
  expect(translation()).toBe(0);
  tap(1);
  expect(onClose).toHaveBeenCalledTimes(1);
});

it('the message gallery steps both ways, respects the ends and the strip switches', async () => {
  const container = document.createElement('div');
  const root = createRoot(container);
  roots.push(root);
  await act(async () => root.render(createElement(ImageThumb, {
    items: [1, 2].map((n) => ({ uri: `https://example.test/${n}.png`, name: `${n}.png`, kind: 'image' as const })),
    index: 0, style: {},
  })));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="abrir 1.png"]')!.click());
  expect(container.textContent).toContain('1/2');
  drag(-100);
  expect(container.textContent).toContain('2/2');
  expect(translation()).toBe(0);
  drag(-100);
  expect(container.textContent).toContain('2/2');
  drag(100);
  expect(container.textContent).toContain('1/2');
  drag(100);
  expect(container.textContent).toContain('1/2');
  const thumbs = [...container.querySelectorAll<HTMLButtonElement>('[aria-label="abrir 2.png"]')];
  await act(async () => thumbs[thumbs.length - 1].click());
  expect(container.textContent).toContain('2/2');
});

it('closing from outside (Android back) and reopening the same image comes back without zoom', async () => {
  const { root, onClose, onStep } = await render();
  deliver('PinchGestureHandler', 'onStart', event(0, 0, 1, 2));
  deliver('PinchGestureHandler', 'onUpdate', event(0, 0, 2, 2));
  deliver('PinchGestureHandler', 'onEnd');
  drag(100, 20);
  const props = { uri: 'https://example.test/1.png', filename: '1.png', onClose, onStep };
  await act(async () => root.render(createElement(Lightbox, { ...props, visible: false })));
  await act(async () => root.render(createElement(Lightbox, { ...props, visible: true })));
  expect(native.style().transform).toEqual([{ translateX: 0 }, { translateY: 0 }, { scale: 1 }]);
});

it('video also has save, and it saves the open video', async () => {
  const { saveImage } = await import('./mediaCache');
  vi.mocked(saveImage).mockResolvedValue(false);
  const { container } = await render({ kind: 'video', uri: 'https://example.test/demo.mp4', filename: 'demo.mp4' });
  const salvar = container.querySelector<HTMLButtonElement>('[aria-label="salvar"]');
  expect(salvar).not.toBeNull();
  await act(async () => salvar!.click());
  expect(saveImage).toHaveBeenCalledWith('https://example.test/demo.mp4', undefined, 'demo.mp4');
});

it('caption, share and "in chat"', async () => {
  const { shareFile } = await import('./mediaCache');
  vi.mocked(shareFile).mockResolvedValue();
  const onShowInChat = vi.fn();
  const headers = { Authorization: 'Bearer t' };
  const { container } = await render({ details: '580 KB · today 11:42 · agent', onShowInChat, headers, counter: '3/8' });
  expect(container.textContent).toContain('580 KB · today 11:42 · agent');
  expect(container.textContent).toContain('3/8');
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="share"]')!.click());
  expect(shareFile).toHaveBeenCalledWith('https://example.test/1.png', headers, '1.png');
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="in_chat"]')!.click());
  expect(onShowInChat).toHaveBeenCalledTimes(1);
});

it('without "in chat" the action is not offered', async () => {
  const { container } = await render();
  expect(container.querySelector('[aria-label="in_chat"]')).toBeNull();
});
