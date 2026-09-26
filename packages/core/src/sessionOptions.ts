// Opções de abertura de sessão, as mesmas do backend (model_args.py). Uma cópia só: a folha de
// nova sessão, a orquestração e o app nativo divergiam quando cada um tinha a sua.
import type { ModelOption } from './api';
import type { Provider } from './types';

export const SESSION_PROVIDERS: readonly Provider[] = ['claude', 'codex', 'pi', 'kimi', 'omp'];

// Sem entrada para o Kimi de propósito: ele escolhe modelo mas o CLI não tem nível de esforço.
// OMP é o fork do Pi, mesmos níveis.
const PI_LEVELS = ['off', 'minimal', 'low', 'medium', 'high', 'xhigh', 'max'];
export const EFFORT_LEVELS: Readonly<Record<string, readonly string[]>> = {
  claude: ['low', 'medium', 'high', 'xhigh', 'max'],
  pi: PI_LEVELS,
  omp: PI_LEVELS,
};

/** Níveis de esforço do provider. Os do Codex são POR MODELO e vêm do provedor; sem modelo
 * escolhido a lista é vazia, porque inventar uma ofereceria nível que o modelo pode não aceitar. */
export function effortLevels(provider: string, models: readonly ModelOption[], model: string): readonly string[] {
  if (provider === 'codex') return models.find((m) => m.id === model || `${m.provider}/${m.id}` === model)?.efforts ?? [];
  return EFFORT_LEVELS[provider] ?? [];
}

// Do mais contido ao mais livre: a ordem é a própria escala de autonomia.
export const CLAUDE_PERMISSION_MODES = [
  'plan', 'manual', 'auto', 'acceptEdits', 'bypassPermissions', 'dontAsk',
] as const;

// Codex sem terminal: o modo vira sandbox/approval, com os nomes do próprio Codex.
export const CODEX_HEADLESS_PERMISSION_MODES = ['Ask for approval', 'Approve for me', 'Full Access'] as const;

/** Só Claude e Codex sem terminal escolhem permissão. */
export function permissionModes(provider: string, headless: boolean): readonly string[] {
  if (provider === 'claude') return CLAUDE_PERMISSION_MODES;
  return provider === 'codex' && headless ? CODEX_HEADLESS_PERMISSION_MODES : [];
}

export const hasExecutionMode = (provider: string) => provider === 'claude' || provider === 'codex';
