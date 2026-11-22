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
vi.mock('./mediaCache', () => ({
  saveImage: vi.fn(), shareFile: vi.fn(), canShareFile: true,
  isAbort: (e: unknown) => e instanceof Error && e.name === 'AbortError',
}));
vi.mock('../../ui/Toast', () => ({ toast: { ok: vi.fn(), erro: vi.fn() } }));
vi.mock('../../ui/Icon', () => ({ Icon: () => null }));
vi.mock('../../paraglide/messages', () => ({
  anexos_fechar_imagem: () => 'fechar', anexos_baixar: () => 'salvar',
  anexos_ver: ({ n }: { n: string }) => `abrir ${n}`, anexos_tocar: ({ nome }: { nome: string }) => `tocar ${nome}`,
  gallery_save: () => 'gallery_save', gallery_share: () => 'gallery_share', gallery_share_named: () => 'share',
  gallery_in_chat: () => 'in_chat', gallery_counter: ({ n, total }: { n: number; total: number }) => `${n}/${total}`,
  visor_anterior: () => 'previous', visor_proxima: () => 'next',
  gallery_video_swipe_hint: () => 'swipe', gallery_video_close_hint: () => 'swipe_close',
  visor_nao_carregou: () => 'did not load', comum_carregando: () => 'loading', arquivo_carregar_erro: () => 'error',
  lista_tentar_novamente: () => 'retry', file_card_cancel: () => 'cancelar',
}));

type FakePlayer = {
  source: unknown; status: string; play: ReturnType<typeof vi.fn>; pause: ReturnType<typeof vi.fn>;
  listeners: Set<(e: { status: string; error?: { message: string } }) => void>;
  addListener: (ev: string, l: (e: { status: string; error?: { message: string } }) => void) => { remove: () => void };
};
const video = vi.hoisted(() => ({ players: [] as FakePlayer[] }));
vi.mock('expo-video', async () => {
  const React = await import('react');
  return {
    useVideoPlayer: (source: unknown, setup?: (p: FakePlayer) => void) => {
      const [p] = React.useState(() => {
        const listeners = new Set<(e: { status: string; error?: { message: string } }) => void>();
        const pl: FakePlayer = {
          source, status: 'idle', play: vi.fn(), pause: vi.fn(), listeners,
          addListener: (_ev, l) => { listeners.add(l); return { remove: () => listeners.delete(l) }; },
        };
        setup?.(pl);
        video.players.push(pl);
        return pl;
      });
      return p;
    },
    VideoView: () => React.createElement('div', { 'data-testid': 'video' }),
    // A miniatura da conversa (ImageThumb) importa; os testes daqui só abrem imagens nela.
    createVideoPlayer: vi.fn(),
  };
});

import { clampPan, Lightbox, panLimits, type LightboxProps } from './Lightbox';
import { ImageThumb } from '../../chat/ImageThumb';
import { useVoiceAudioState } from '../voice/audioSession';

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
  // Sem prazo curto, o toque simples que fecha esperava o padrão do sistema para o segundo toque.
  expect(double.config).toMatchObject({ maxDelayMs: 250 });
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

it('the zoomed pan stops at the edge of the scaled image', () => {
  // Caixa 400×800, foto 1000×500 (aparece 400×200). Em 2×: 800×400, sobram 200 de cada lado e nada na vertical.
  expect(panLimits(400, 800, 1000, 500, 2)).toEqual({ x: 200, y: 0 });
  // Antes de a foto carregar, vale a caixa inteira.
  expect(panLimits(400, 800, 0, 0, 2)).toEqual({ x: 200, y: 400 });
  // Caixa sem medida não limita.
  expect(panLimits(0, 0, 1000, 500, 2)).toEqual({ x: Infinity, y: Infinity });
  expect(clampPan(350, 200)).toBe(200);
  expect(clampPan(-350, 200)).toBe(-200);
  expect(clampPan(120, 200)).toBe(120);
  expect(clampPan(50, 0)).toBe(0);
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
  // A miniatura carrega o visor sob demanda: espera o módulo chegar e o Suspense mostrar o visor.
  await act(async () => {
    await import('./gallery');
    await new Promise((resolve) => setTimeout(resolve, 0));
  });
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
  expect(saveImage).toHaveBeenCalledWith('https://example.test/demo.mp4', undefined, 'demo.mp4',
    expect.objectContaining({ signal: expect.any(AbortSignal) }));
});

it('salvar mostra o anel enquanto baixa, e tocar nele cancela', async () => {
  const { saveImage } = await import('./mediaCache');
  let signal: AbortSignal | undefined;
  vi.mocked(saveImage).mockImplementation((_u, _h, _n, o) => new Promise((_res, rej) => {
    signal = o?.signal;
    o?.signal?.addEventListener('abort', () => { const e = new Error('x'); e.name = 'AbortError'; rej(e); });
  }));
  const { toast } = await import('../../ui/Toast');
  const { container } = await render();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="salvar"]')!.click());
  expect(container.querySelector('[aria-label="salvar"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="cancelar"]')!.click());
  expect(signal?.aborted).toBe(true);
  expect(container.querySelector('[aria-label="salvar"]')).not.toBeNull();
  expect(toast.erro).not.toHaveBeenCalled();
});

it('passar de mídia ou fechar o visor cancela o download em andamento', async () => {
  const { saveImage } = await import('./mediaCache');
  const signals: AbortSignal[] = [];
  vi.mocked(saveImage).mockImplementation((_u, _h, _n, o) => new Promise((_res, rej) => {
    signals.push(o!.signal!);
    o?.signal?.addEventListener('abort', () => { const e = new Error('x'); e.name = 'AbortError'; rej(e); });
  }));
  const { container, root, onClose, onStep } = await render();
  const props = { filename: '1.png', onClose, onStep, visible: true };
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="salvar"]')!.click());
  await act(async () => root.render(createElement(Lightbox, { ...props, uri: 'https://example.test/2.png' })));
  expect(signals[0].aborted).toBe(true);
  // O anel não passa para a próxima mídia.
  expect(container.querySelector('[aria-label="cancelar"]')).toBeNull();
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="salvar"]')!.click());
  await act(async () => root.render(createElement(Lightbox, { ...props, uri: 'https://example.test/2.png', visible: false })));
  expect(signals[1].aborted).toBe(true);
});

it('vídeo do servidor toca pelo player com o token no header, sem baixar antes', async () => {
  video.players.length = 0;
  const { saveImage } = await import('./mediaCache');
  vi.mocked(saveImage).mockClear();
  const headers = { Authorization: 'Bearer t' };
  const { container } = await render({ kind: 'video', uri: 'https://srv/api/f.mp4', filename: 'f.mp4', headers });
  expect(container.querySelector('[data-testid="video"]')).not.toBeNull();
  expect(video.players).toHaveLength(1);
  expect(video.players[0].source).toEqual({ uri: 'https://srv/api/f.mp4', headers, useCaching: true });
  expect(video.players[0].play).toHaveBeenCalledTimes(1);
  expect(saveImage).not.toHaveBeenCalled();
  // Erro do player aparece com o motivo, não some calado.
  act(() => video.players[0].listeners.forEach((l) => l({ status: 'error', error: { message: 'HTTP 403' } })));
  expect(container.textContent).toContain('did not load');
  expect(container.textContent).toContain('HTTP 403');
});

it('voz ao vivo pausa o vídeo aberto', async () => {
  video.players.length = 0;
  useVoiceAudioState.setState({ voiceActive: false });
  await render({ kind: 'video', uri: 'https://srv/api/f.mp4', filename: 'f.mp4' });
  act(() => useVoiceAudioState.setState({ voiceActive: true }));
  expect(video.players[0].pause).toHaveBeenCalled();
  useVoiceAudioState.setState({ voiceActive: false });
});

it('endereço que não é http(s) não chega ao player', async () => {
  video.players.length = 0;
  const { container } = await render({ kind: 'video', uri: 'file:///etc/x.mp4', filename: 'x.mp4' });
  expect(video.players).toHaveLength(0);
  expect(container.textContent).toContain('did not load');
});

it('video steps by the buttons and the strip below the player, which also closes', async () => {
  const { container, onStep, onClose } = await render({ kind: 'video', uri: 'https://example.test/demo.mp4', filename: 'demo.mp4' });
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="next"]')!.click());
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="previous"]')!.click());
  expect(onStep.mock.calls).toEqual([[1], [-1]]);
  drag(-100);
  expect(onStep).toHaveBeenLastCalledWith(1);
  drag(5, 200);
  expect(onClose).toHaveBeenCalledTimes(1);
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
  expect(shareFile).toHaveBeenCalledWith('https://example.test/1.png', headers, '1.png',
    expect.objectContaining({ signal: expect.any(AbortSignal) }));
  await act(async () => container.querySelector<HTMLButtonElement>('[aria-label="in_chat"]')!.click());
  expect(onShowInChat).toHaveBeenCalledTimes(1);
});

it('an image that does not load says so and retries; another image starts clean', async () => {
  const { container, root } = await render();
  const fail = () => act(() => { container.querySelector('img')!.dispatchEvent(new Event('error')); });
  fail();
  expect(container.textContent).toContain('error');
  const before = container.querySelector('img');
  await act(async () => container.querySelector<HTMLElement>('[aria-label="retry: 1.png"]')!.click());
  expect(container.textContent).not.toContain('error');
  expect(container.querySelector('img')).not.toBe(before);
  fail();
  expect(container.textContent).toContain('error');
  await act(async () => root.render(createElement(Lightbox, {
    visible: true, uri: 'https://example.test/2.png', filename: '2.png', onClose: vi.fn(), onStep: vi.fn(),
  })));
  expect(container.textContent).not.toContain('error');
});

it('without "in chat" the action is not offered', async () => {
  const { container } = await render();
  expect(container.querySelector('[aria-label="in_chat"]')).toBeNull();
});
