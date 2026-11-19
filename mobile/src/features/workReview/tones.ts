import type { useUnistyles } from 'react-native-unistyles';
import type { WorkReviewBadgeTone, WorkReviewTone } from '@hangar/core';
import type { Tone } from '../../ui/Chip';

type Theme = ReturnType<typeof useUnistyles>['theme'];

/** Cor forte e fundo de cada tom do estado: running = destaque, waiting = âmbar, applying/done = verde. */
export function toneColors(theme: Theme, tone: WorkReviewTone): { fg: string; surface: string } {
  if (tone === 'accent') return { fg: theme.tokens.accent.base, surface: theme.tokens.pill.working.bg };
  if (tone === 'warning') return { fg: theme.tokens.status.warning, surface: theme.tokens.pill.input.bg };
  return { fg: theme.tokens.status.success, surface: theme.tokens.pill.idle.bg };
}

export function badgeColor(theme: Theme, tone: WorkReviewBadgeTone): string {
  return tone === 'warning' ? theme.tokens.status.warning : tone === 'success' ? theme.tokens.status.success : theme.tokens.accent.base;
}

export const BADGE_CHIP_TONE: Record<WorkReviewBadgeTone, Tone> = { warning: 'warning', success: 'success', info: 'accent' };
