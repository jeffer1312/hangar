import type { ComponentType } from 'react';
import { RenameSuggestionAnimation } from './RenameSuggestionAnimation';
import { TagsPlateAnimation } from './TagsPlateAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'etiquetas-maquina-conta': TagsPlateAnimation,
  'sugestao-de-nome': RenameSuggestionAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
