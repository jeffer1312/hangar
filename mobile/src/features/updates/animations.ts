import type { ComponentType } from 'react';
import { AutoReportAnimation } from './AutoReportAnimation';
import { CodexActivityAnimation } from './CodexActivityAnimation';
import { HideFromListAnimation } from './HideFromListAnimation';
import { RenameSuggestionAnimation } from './RenameSuggestionAnimation';
import { TagsPlateAnimation } from './TagsPlateAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'codex-atividade': CodexActivityAnimation,
  'etiquetas-maquina-conta': TagsPlateAnimation,
  'ocultar-da-lista': HideFromListAnimation,
  'relatorio-automatico': AutoReportAnimation,
  'sugestao-de-nome': RenameSuggestionAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
