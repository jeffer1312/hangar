import { ScheduledTasksAnimation } from './ScheduledTasksAnimation';
import { ReviewsAnimation } from './reviews-animation';
import type { ComponentType } from 'react';
import { AutoReportAnimation } from './AutoReportAnimation';
import { CodexActivityAnimation } from './CodexActivityAnimation';
import { CompactIdleAnimation } from './CompactIdleAnimation';
import { DesktopToolLookAnimation } from './DesktopToolLookAnimation';
import { ConnectAnimation } from './ConnectAnimation';
import { FolderBranchAnimation } from './FolderBranchAnimation';
import { HideFromListAnimation } from './HideFromListAnimation';
import { ImageRevealAnimation } from './ImageRevealAnimation';
import { LoginPasswordAnimation } from './LoginPasswordAnimation';
import { NoProjectAnimation } from './NoProjectAnimation';
import { PluginsAnimation } from './PluginsAnimation';
import { RenameSuggestionAnimation } from './RenameSuggestionAnimation';
import { SleepHiddenAnimation } from './SleepHiddenAnimation';
import { SessionVaultAnimation } from './SessionVaultAnimation';
import { TagsPlateAnimation } from './TagsPlateAnimation';
import { UpdatesManualAnimation } from './UpdatesManualAnimation';
import { VaultAnimation } from './VaultAnimation';
import { WorktreeFreshAnimation } from './WorktreeFreshAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'revisoes-mobile': ReviewsAnimation,
  'scheduled-tasks': ScheduledTasksAnimation,
  'adormecer-ocultas': SleepHiddenAnimation,
  'atualizacoes-e-manual': UpdatesManualAnimation,
  'branch-no-celular': FolderBranchAnimation,
  'cofre-de-senhas': VaultAnimation,
  'codex-atividade': CodexActivityAnimation,
  'compactar-parada': CompactIdleAnimation,
  'estilo-claude-desktop': DesktopToolLookAnimation,
  'etiquetas-maquina-conta': TagsPlateAnimation,
  'hangar-connect': ConnectAnimation,
  'login-usuario-senha': LoginPasswordAnimation,
  'ocultar-da-lista': HideFromListAnimation,
  'plugins-do-claude': PluginsAnimation,
  'relatorio-automatico': AutoReportAnimation,
  'revelacao-imagem': ImageRevealAnimation,
  'pedido-na-sessao': SessionVaultAnimation,
  'sem-projeto': NoProjectAnimation,
  'sugestao-de-nome': RenameSuggestionAnimation,
  'worktree-atualizada': WorktreeFreshAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
