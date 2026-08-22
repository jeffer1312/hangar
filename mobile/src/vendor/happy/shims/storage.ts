// Shim para '@/sync/storage' — satisfaz useSetting/useLocalSetting/storage do Happy
import { prefs } from '../../../stores/prefs';

const DEFAULTS = {
  compactToolCalls: false,
  showLineNumbersInToolViews: true,
  wrapLinesInDiffs: true,
  devModeEnabled: false,
} as const;

// Fallback quando rodando em Node (vitest) onde prefs (mmkv) não funciona
function safeGetBoolean(key: string, fallback: boolean): boolean {
  try {
    const v = (prefs as unknown as { getBoolean?: (k: string) => boolean | undefined })?.getBoolean?.(key);
    return v ?? fallback;
  } catch {
    return fallback;
  }
}

export function useSetting<K extends keyof typeof DEFAULTS>(key: K): (typeof DEFAULTS)[K] {
  // prefs é síncrono (mmkv). Em Node retorna fallback.
  return safeGetBoolean(key as string, DEFAULTS[key]) as (typeof DEFAULTS)[K];
}

export function useLocalSetting<K extends keyof typeof DEFAULTS>(key: K): (typeof DEFAULTS)[K] {
  return useSetting(key);
}

// stub para `import { storage } from '@/sync/storage'` (PermissionFooter já não usa em runtime aqui, mas mantém API)
export const storage: {
  getState: () => { sessions: Record<string, unknown> };
  getStateRaw?: unknown;
} = {
  getState: () => ({ sessions: {} }),
};
