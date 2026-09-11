import { describe, expect, it } from 'vitest';
import { aguardarSwNovo } from './swNovo';

type Ouvinte = () => void;

function workerFake(estado: ServiceWorkerState) {
  const ouvintes: Ouvinte[] = [];
  const w = {
    state: estado,
    addEventListener: (_: string, fn: Ouvinte) => { ouvintes.push(fn); },
    mudar(novo: ServiceWorkerState) { w.state = novo; ouvintes.forEach((f) => f()); },
  };
  return w;
}

function regFake(opts: { instalando?: ReturnType<typeof workerFake> | null; aoUpdate?: (r: any) => void } = {}) {
  const ouvintes: Record<string, Ouvinte[]> = {};
  const r: any = {
    installing: null, waiting: null, active: {},
    addEventListener: (ev: string, fn: Ouvinte) => { (ouvintes[ev] ??= []).push(fn); },
    update: async () => {
      opts.aoUpdate?.(r);
      if (opts.instalando !== undefined) {
        r.installing = opts.instalando;
        (ouvintes.updatefound ?? []).forEach((f) => f());
      }
    },
  };
  return r;
}

describe('aguardarSwNovo', () => {
  it('sem registro (aba sem SW) resolve na hora', async () => {
    expect(await aguardarSwNovo(null)).toBe(true);
  });

  it('sem worker novo depois do update: o SW ativo já é o do build', async () => {
    expect(await aguardarSwNovo(regFake())).toBe(true);
  });

  it('espera o worker novo sair de installing até activated — é o que o reload precisava', async () => {
    const w = workerFake('installing');
    const p = aguardarSwNovo(regFake({ instalando: w }));
    let resolveu = false;
    void p.then(() => { resolveu = true; });
    await Promise.resolve(); await Promise.resolve();
    expect(resolveu).toBe(false);          // ainda instalando: NÃO pode recarregar
    w.mudar('installed'); w.mudar('activating');
    await Promise.resolve();
    expect(resolveu).toBe(false);
    w.mudar('activated');
    expect(await p).toBe(true);
  });

  it('worker novo que vira redundant (instalação falhou) devolve false', async () => {
    const w = workerFake('installing');
    const p = aguardarSwNovo(regFake({ instalando: w }));
    w.mudar('redundant');
    expect(await p).toBe(false);
  });

  it('pega o installing anunciado pelo updatefound mesmo se reg.installing for limpo antes de ler', async () => {
    const w = workerFake('activated');
    const reg = regFake({ instalando: w, aoUpdate: (r) => { setTimeout(() => { r.installing = null; }, 0); } });
    expect(await aguardarSwNovo(reg)).toBe(true);
  });
});
