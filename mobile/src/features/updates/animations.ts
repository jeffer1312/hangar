import type { ComponentType } from 'react';
import { CodexActivityAnimation } from './CodexActivityAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'codex-atividade': CodexActivityAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
