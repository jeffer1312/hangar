// Glifo e rótulo curto de um modo de permissão do Claude Code, pra pill e pro seletor. Os glifos
// são os do rodapé do próprio Claude: ⏸ nos modos que param pra perguntar, ⏵⏵ nos que seguem.
import * as m from '../paraglide/messages';

const GLIFO: Record<string, string> = {
  plan: '⏸',
  manual: '⏸',
  auto: '⏵⏵',
  acceptEdits: '⏵⏵',
  bypassPermissions: '⏵⏵',
  dontAsk: '⏵⏵',
};

const ROTULO: Record<string, () => string> = {
  plan: () => m.permissao_modo_plan(),
  manual: () => m.permissao_modo_manual(),
  auto: () => m.permissao_modo_auto(),
  acceptEdits: () => m.permissao_modo_acceptEdits(),
  bypassPermissions: () => m.permissao_modo_bypassPermissions(),
  dontAsk: () => m.permissao_modo_dontAsk(),
};

// Consequência prática de cada modo, uma frase. Fica ao lado do rótulo, dentro do próprio item:
// tooltip não existe no celular, e sem isto a lista pedia que a pessoa adivinhasse a diferença
// entre "Auto" e "Aceitar edições" pelo nome.
const DESCRICAO: Record<string, () => string> = {
  plan: () => m.permissao_desc_plan(),
  manual: () => m.permissao_desc_manual(),
  auto: () => m.permissao_desc_auto(),
  acceptEdits: () => m.permissao_desc_acceptEdits(),
  bypassPermissions: () => m.permissao_desc_bypassPermissions(),
  dontAsk: () => m.permissao_desc_dontAsk(),
};

// Do mais contido ao mais livre — a ordem é a própria escala de autonomia, e é o que deixa a
// pessoa parar de ler quando chegou no ponto que queria.
export const MODOS_PERMISSAO = [
  'plan', 'manual', 'auto', 'acceptEdits', 'bypassPermissions', 'dontAsk',
] as const;

// Os dois últimos da escala: sem confirmação nenhuma. Ganham tinta mais quente no glifo — não
// fundo de alerta, que fica reservado a erro; modo escolhido de propósito não é erro.
export function permissaoSemFreio(modo: string): boolean {
  return modo === 'bypassPermissions' || modo === 'dontAsk';
}

export function glifoPermissao(modo: string): string {
  return GLIFO[modo] ?? '';
}


/** Vazio pro modo que o app não conhece: melhor sem frase que com frase inventada. */
export function descricaoPermissao(modo: string): string {
  return DESCRICAO[modo]?.() ?? '';
}

// Modo que o app não conhece (CLI novo) sai como o id cru: dado, não interface.
export function rotuloPermissao(modo: string): string {
  return ROTULO[modo]?.() ?? modo;
}
