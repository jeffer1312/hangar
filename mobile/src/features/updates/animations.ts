import type { ComponentType } from 'react';
import { AutoReportAnimation } from './AutoReportAnimation';
import { CodexActivityAnimation } from './CodexActivityAnimation';
import { RenameSuggestionAnimation } from './RenameSuggestionAnimation';
import { TagsPlateAnimation } from './TagsPlateAnimation';
import { VaultAnimation } from './VaultAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'cofre-de-senhas': VaultAnimation,
  'codex-atividade': CodexActivityAnimation,
  'etiquetas-maquina-conta': TagsPlateAnimation,
  'relatorio-automatico': AutoReportAnimation,
  'sugestao-de-nome': RenameSuggestionAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
