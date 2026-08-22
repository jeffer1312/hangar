import type { ThemeTokens } from '@hangar/core';

export function happyColors(t: ThemeTokens) {
  return {
    text: t.text.primary,
    textSecondary: t.text.secondary,
    textDestructive: t.status.error,
    divider: t.border.default,
    surface: t.bg.surface,
    surfaceHigh: t.bg.elevated,
    surfaceHighest: t.bg.hover,
    warning: t.status.warning,
    success: t.status.success,
    radio: { active: t.accent.base, dot: t.text.inverse },
    box: {
      error: { text: t.status.error, border: t.status.error, background: t.glass.border },
      warning: { text: t.status.warning },
    },
    input: { text: t.text.primary, placeholder: t.text.muted },
    header: { tint: t.accent.base },
    button: { primary: { tint: t.text.inverse, background: t.accent.base } },
    groupped: { background: t.bg.base },
    diff: {
      outline: t.border.default,
      success: t.status.success,
      error: t.status.error,
      addedBg: 'rgba(52,199,89,0.14)',
      addedBorder: t.status.success,
      addedText: t.text.primary,
      removedBg: 'rgba(255,69,58,0.14)',
      removedBorder: t.status.error,
      removedText: t.text.primary,
      contextBg: t.bg.surface,
      contextText: t.text.secondary,
      // usados direto por DiffView/PierreDiffView
      leadingSpaceDot: t.border.subtle,
      inlineAddedBg: 'rgba(52,199,89,0.28)',
      inlineAddedText: t.text.primary,
      inlineRemovedBg: 'rgba(255,69,58,0.28)',
      inlineRemovedText: t.text.primary,
      hunkHeaderBg: t.bg.elevated,
      hunkHeaderText: t.text.secondary,
      lineNumberBg: t.bg.elevated,
      lineNumberText: t.text.muted,
    },
    // CommandView usa theme.colors.terminal.*
    terminal: {
      background: '#1e1e1e',
      prompt: '#34c759',
      command: '#e0e0e0',
      stdout: '#e0e0e0',
      stderr: '#ffb86c',
      error: '#ff5555',
      emptyOutput: '#6272a4',
    },
  };
}
