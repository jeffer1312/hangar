import { registrarDiag, type Server } from '@hangar/core';

// `dictation.client_timing` do ditado da conversa e da tela inicial. `save`: o caminho do áudio subido
// gravado no aparelho; `finish`: o texto entregue ao campo ou ao rascunho.
type Stage = 'check' | 'upload' | 'save' | 'transcribe' | 'finish';
const STAGES: Stage[] = ['check', 'upload', 'save', 'transcribe', 'finish'];

// Etapas contíguas: a soma delas com a parada → arquivo e o preparo é o total, e a falha cai na etapa em curso.
export function stageClock() {
  const ms: Record<Stage, number> = { check: 0, upload: 0, save: 0, transcribe: 0, finish: 0 };
  let stage: Stage = 'check';
  let at = Date.now();
  const close = () => { const now = Date.now(); ms[stage] += now - at; at = now; };
  return {
    ms, start: at,
    get stage() { return stage; },
    enter(next: Stage) { close(); stage = next; },
    close,
  };
}

export const failureCode = (detail: string) => /^(\d{3}):/.exec(detail)?.[1] ?? 'failed';

/**
 * O diário do servidor só aceita campos conhecidos (`_CAMPOS` do Python): `etapa` e `motivo` vão como campos, os
 * tempos no `detalhe`. `motivo` diz se a parada foi um toque, o silêncio, o teto ou a tela escondida.
 * `stop_to_file_ms` vai da parada ao arquivo pronto no gravador, como no nativo; `prepare_ms`, a cópia e o
 * registro do áudio antes do envio. Sem `fileAt`, todo o intervalo conta como `stop_to_file_ms`.
 * `failure.expected`: recusa prevista (app escondido, outra conversa), que é aviso.
 */
export function recordTiming(server: Server, stop: { stoppedAt: number; fileAt?: number }, clock: ReturnType<typeof stageClock>,
  motivo: string, failure?: { code: string; expected?: boolean }, warned = false): void {
  clock.close();
  const total = Date.now() - stop.stoppedAt;
  const fileAt = stop.fileAt ?? clock.start;
  const steps = STAGES.map((s) => `${s}_ms=${clock.ms[s]}`).join(' ');
  const nivel = failure ? failure.expected ? 'aviso' : 'erro' : warned ? 'aviso' : 'ok';
  registrarDiag({
    evento: 'dictation.client_timing', nivel, ...(failure ? { codigo: failure.code } : {}), ms: total,
    ...(failure ? { etapa: clock.stage } : {}), motivo,
    detalhe: `stop_to_file_ms=${fileAt - stop.stoppedAt} prepare_ms=${clock.start - fileAt} ${steps} total_ms=${total}`,
  }, server.invite ? undefined : server.baseUrl);
}
