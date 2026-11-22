import * as React from 'react';

type Listener = (e: { status: string; error?: { message: string } }) => void;

// Player inerte: guarda a fonte e os listeners para o teste disparar `statusChange` à mão.
export function createVideoPlayer(source: unknown) {
  const listeners = new Set<Listener>();
  return {
    source,
    status: 'idle',
    play() {},
    pause() {},
    release() {},
    generateThumbnailsAsync: async (_times: number | number[], _options?: object) => [] as unknown[],
    addListener(_event: 'statusChange', l: Listener) { listeners.add(l); return { remove: () => listeners.delete(l) }; },
    emit(e: Parameters<Listener>[0]) { listeners.forEach((l) => l(e)); },
  };
}

export function useVideoPlayer(source: unknown, setup?: (p: ReturnType<typeof createVideoPlayer>) => void) {
  const [player] = React.useState(() => {
    const p = createVideoPlayer(source);
    setup?.(p);
    return p;
  });
  return player;
}

export const VideoView = (props: { player: unknown }) => React.createElement('div', { 'data-testid': 'video', 'data-player': String(!!props.player) });
export const getCurrentVideoCacheSize = () => 0;
export const clearVideoCacheAsync = async () => {};
export const setVideoCacheSizeAsync = async (_bytes: number) => {};
export type VideoPlayerStatus = 'idle' | 'loading' | 'readyToPlay' | 'error';
export type VideoThumbnail = { width: number; height: number };
