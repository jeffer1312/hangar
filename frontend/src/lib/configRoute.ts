// frontend/src/lib/configRoute.ts
// O painel de Configuracoes vive num SEGUNDO eixo do endereco: o caminho antes do `?` continua
// escolhendo a tela de tras (chat, quadro, lista), e a query diz qual tela do painel esta por cima.
// Assim o painel abre sobre QUALQUER rota sem que nenhuma delas precise saber que ele existe.

export type TelaConfig = 'root' | 'geral' | 'aparencia' | 'notificacoes' | 'anexos' | 'avancado' | 'motores' | 'sobre' | 'diario' | 'servidores' | 'acesso' | 'contas' | 'orquestracao' | 'voz' | 'harnesses';

const TELAS: readonly TelaConfig[] = ['root', 'geral', 'aparencia', 'notificacoes', 'anexos', 'avancado', 'motores', 'sobre', 'diario', 'servidores', 'acesso', 'contas', 'orquestracao', 'voz', 'harnesses'];

// Rotas de tela que mudaram de nome: um link guardado por alguém não pode virar tela em branco.
const RENOMEADAS: Record<string, TelaConfig> = { ditado: 'voz' };

/** Telas que mexem NUM servidor — sem alvo resolvido elas nao podem abrir (ver App.svelte). */
export const TELAS_DE_SERVIDOR: readonly TelaConfig[] = ['acesso', 'contas', 'notificacoes', 'anexos', 'avancado', 'motores', 'orquestracao', 'voz', 'harnesses'];

export interface RotaConfig {
  tela: TelaConfig;
  /** ID do servidor alvo. So o ID: guardar o objeto prende base/token no que valia ao abrir. */
  srv: string | null;
}

export function parseConfig(hash: string): RotaConfig | null {
  const q = hash.indexOf('?');
  if (q < 0) return null;
  const p = new URLSearchParams(hash.slice(q + 1));
  const tela = p.get('config');
  // Tela desconhecida = painel fechado, caminho intacto. Nao adivinhar: um `?config=aparenca` com
  // typo abrindo a Aparencia esconderia o erro do link.
  const alvo = RENOMEADAS[tela ?? ''] ?? tela;
  if (!alvo || !TELAS.includes(alvo as TelaConfig)) return null;
  return { tela: alvo as TelaConfig, srv: p.get('srv') || null };
}

export function comConfig(hash: string, tela: TelaConfig | null, srv?: string | null): string {
  const caminho = (hash.split('?')[0] || '#/');
  if (!tela) return caminho;
  const p = new URLSearchParams();
  p.set('config', tela);
  if (srv) p.set('srv', srv);
  return `${caminho}?${p.toString()}`;
}
