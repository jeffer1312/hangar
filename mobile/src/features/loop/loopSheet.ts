import type { LoopState } from '@hangar/core';
import * as m from '../../paraglide/messages';

const FINAL = new Set<LoopState['status']>(['done', 'stopped', 'exhausted', 'failed']);

export function isFinal(loop: LoopState | null | undefined): boolean {
  return !!loop && FINAL.has(loop.status);
}

export function isForm(loop: LoopState | null | undefined, forceForm: boolean): boolean {
  return !loop || (isFinal(loop) && forceForm);
}

export function isPolling(loop: LoopState | null | undefined): boolean {
  return !!loop && !isFinal(loop);
}

export function cleanErr(error: unknown): string {
  const message = error instanceof Error ? error.message : m.preview_falhou();
  if (/failed to fetch|networkerror|load failed|timed? ?out/i.test(message)) {
    return m.loop_servidor_nao_respondeu();
  }
  return message.replace(/^\d+:\s*/, '');
}
