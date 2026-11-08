import { describe, expect, it } from 'vitest';
import { readFileSync } from 'node:fs';
import { join } from 'node:path';
import { textoProblema } from './problema';
import * as m from '../paraglide/messages';

// Códigos que o estado do Rust publica sem passar pelo Python.
const NOVOS = ['state_facts_unavailable', 'permission_observe_failed', 'interacao_indisponivel', 'limit_switch_no_room', 'limit_switch_failed',
  'limit_switch_no_target', 'terminal_sem_memoria', 'terminal_caiu'];

describe('textoProblema', () => {
  it('traduz os códigos do estado do Rust', () => {
    for (const codigo of NOVOS) expect(textoProblema(codigo), codigo).toBeTruthy();
  });

  it('mensagem parada com a sessão surda diz que não foi entregue', () => {
    expect(textoProblema('headless_delivery_stalled')).toBe(m.problema_headless_delivery_stalled());
  });

  it('código desconhecido some', () => {
    expect(textoProblema('codigo_sem_frase')).toBeNull();
  });

  // O nativo procura `problema_<código>` nestes arquivos (`problem_banner`, desktop-native/src/app.rs).
  it('as frases existem nas duas línguas para o nativo', () => {
    for (const lingua of ['pt', 'en']) {
      const msgs = JSON.parse(readFileSync(join(import.meta.dirname, '..', '..', '..', 'messages', `${lingua}.json`), 'utf8'));
      for (const codigo of NOVOS) expect(msgs[`problema_${codigo}`], `${lingua}: ${codigo}`).toBeTruthy();
    }
  });
});
