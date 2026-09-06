import type { AggSession } from './types';
import { projectKey, projectLabel, sortSessions, type GroupBy } from './format';

export interface GrupoSessoes {
  id: string;
  label: string;
  color: string | null;
  sessions: AggSession[];
}

// Agrupamento da lista de sessões (toggle Nenhum|Servidor|Projeto). Ordena UMA vez com
// sortSessions e depois só reparte: assim a ordem dentro de cada grupo e a ordem dos próprios
// grupos (nascem na posição do 1º membro) saem da mesma regra da lista lisa.
export function agruparSessoes(rows: AggSession[], modo: GroupBy): GrupoSessoes[] {
  const ordenadas = sortSessions(rows);
  if (modo === 'none') return [{ id: 'todas', label: '', color: null, sessions: ordenadas }];
  const por = new Map<string, GrupoSessoes>();
  for (const r of ordenadas) {
    const id = modo === 'server' ? r.serverId : projectKey(r.cwd);
    const g = por.get(id) ?? {
      id,
      label: modo === 'server' ? r.serverLabel : projectLabel(r.cwd),
      color: modo === 'server' ? r.serverColor : null,
      sessions: [],
    };
    g.sessions.push(r);
    por.set(id, g);
  }
  return [...por.values()];
}
