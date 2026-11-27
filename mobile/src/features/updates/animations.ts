import { ScheduledTasksAnimation } from './ScheduledTasksAnimation';
import { ReviewsAnimation } from './reviews-animation';
import type { ComponentType } from 'react';
import { AutoReportAnimation } from './AutoReportAnimation';
import { ClaudeNoticesAnimation } from './ClaudeNoticesAnimation';
import { ClaudeProvidersAnimation } from './ClaudeProvidersAnimation';
import { CliproxyWizardAnimation } from './CliproxyWizardAnimation';
import { SpeedChoiceAnimation } from './SpeedChoiceAnimation';
import { ProxyUpdateAnimation } from './ProxyUpdateAnimation';
import { AdoptProxyAnimation } from './AdoptProxyAnimation';
import { AntigravityAnimation } from './AntigravityAnimation';
import { AntigravityTermsAnimation } from './AntigravityTermsAnimation';
import { NoticeActionsAnimation } from './NoticeActionsAnimation';
import { CodexActivityAnimation } from './CodexActivityAnimation';
import { CompactIdleAnimation } from './CompactIdleAnimation';
import { DesktopToolLookAnimation } from './DesktopToolLookAnimation';
import { ConnectAnimation } from './ConnectAnimation';
import { ConnectSignInAnimation } from './ConnectSignInAnimation';
import { ConversationGalleryAnimation } from './ConversationGalleryAnimation';
import { DreamAnimation } from './DreamAnimation';
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
import { SessionSettingsAnimation } from './SessionSettingsAnimation';
import { StatusAnimation } from './StatusAnimation';
import { WorkReviewAnimation } from './WorkReviewAnimation';
import { WhatsAppReadonlyAnimation } from './WhatsAppReadonlyAnimation';
import { WhatsAppDraftAnimation } from './WhatsAppDraftAnimation';
import { PortsAnimation } from './PortsAnimation';
import { TelemetryConsentAnimation } from './TelemetryConsentAnimation';
import { AutoContinueAnimation } from './AutoContinueAnimation';
import { LinkedChildrenAnimation } from './LinkedChildrenAnimation';

// Miniaturas feitas dos componentes reais (ex.: src/chat/ComposerField.tsx) entram aqui pelo nome da novidade.
const animations: Record<string, ComponentType<{ width: number }>> = {
  'ajustes-sessao-abas': SessionSettingsAnimation,
  antigravity: AntigravityAnimation,
  'termos-antigravity': AntigravityTermsAnimation,
  'provedores-do-claude': ClaudeProvidersAnimation,
  'revisoes-mobile': ReviewsAnimation,
  'scheduled-tasks': ScheduledTasksAnimation,
  'adormecer-ocultas': SleepHiddenAnimation,
  'avisos-do-claude': ClaudeNoticesAnimation,
  'acoes-dos-avisos': NoticeActionsAnimation,
  'atualizacoes-e-manual': UpdatesManualAnimation,
  'branch-no-celular': FolderBranchAnimation,
  'cliproxy-assistente': CliproxyWizardAnimation,
  'velocidade-ultra-fast': SpeedChoiceAnimation,
  'atualizar-cliproxy': ProxyUpdateAnimation,
  'entregar-cliproxy': AdoptProxyAnimation,
  'cofre-de-senhas': VaultAnimation,
  'codex-atividade': CodexActivityAnimation,
  'compactar-parada': CompactIdleAnimation,
  'estilo-claude-desktop': DesktopToolLookAnimation,
  'etiquetas-maquina-conta': TagsPlateAnimation,
  'galeria-da-conversa': ConversationGalleryAnimation,
  'hangar-connect': ConnectAnimation,
  'entrar-pelo-connect': ConnectSignInAnimation,
  'login-usuario-senha': LoginPasswordAnimation,
  'ocultar-da-lista': HideFromListAnimation,
  'plugins-do-claude': PluginsAnimation,
  'relatorio-automatico': AutoReportAnimation,
  'revisao-do-trabalho': WorkReviewAnimation,
  'revelacao-imagem': ImageRevealAnimation,
  'pedido-na-sessao': SessionVaultAnimation,
  'sem-projeto': NoProjectAnimation,
  'sonho-da-memoria': DreamAnimation,
  'sugestao-de-nome': RenameSuggestionAnimation,
  'tela-de-status': StatusAnimation,
  'worktree-atualizada': WorktreeFreshAnimation,
  'whatsapp-somente-leitura': WhatsAppReadonlyAnimation,
  'whatsapp-draft': WhatsAppDraftAnimation,
  'portas-da-sessao': PortsAnimation,
  'contagens-anonimas': TelemetryConsentAnimation,
  'continuar-depois-da-queda': AutoContinueAnimation,
  'filhas-ligadas': LinkedChildrenAnimation,
};

export const animationFor = (name: string | null): ComponentType<{ width: number }> | null =>
  name ? animations[name] ?? null : null;
