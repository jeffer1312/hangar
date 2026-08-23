import * as m from './paraglide/messages.js';

export type EstiloDitado = 'limpar' | 'prosa' | 'briefing';

export function estilosDitado(): { valor: EstiloDitado; rotulo: string; hint: string }[] {
  return [
    { valor: 'limpar', rotulo: m.ditado_estilo_limpar(), hint: m.ditado_estilo_limpar_hint() },
    { valor: 'prosa', rotulo: m.ditado_estilo_prosa(), hint: m.ditado_estilo_prosa_hint() },
    { valor: 'briefing', rotulo: m.ditado_estilo_briefing(), hint: m.ditado_estilo_briefing_hint() },
  ];
}

export function ehEstilo(v: unknown): v is EstiloDitado {
  return v === 'limpar' || v === 'prosa' || v === 'briefing';
}
