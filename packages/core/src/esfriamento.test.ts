import { describe, it, expect, beforeEach, afterEach, vi } from 'vitest';
import {
  definirArmazem, definirProtegido, estaDesligado, esquecerServidor, registrarFalha, registrarSucesso, retentarAgora,
  _limparEsfriamentoParaTestes,
  endOutage, onServerRecovered, respondeuRecentemente, retryAfterMs, SERVIDOR_CANDIDATO, startOutage,
} from './esfriamento';

beforeEach(() => { _limparEsfriamentoParaTestes(); definirProtegido(() => false); });
afterEach(() => { vi.useRealTimers(); });

describe('servidor desligado', () => {
  it('servidor protegido (o ativo) nunca é marcado, venha a falha de onde vier', () => {
    definirProtegido((id) => id === 'ativo');
    registrarFalha('ativo');
    registrarFalha('outro');
    expect(estaDesligado('ativo')).toBe(false);
    expect(estaDesligado('outro')).toBe(true);
  });

  it('o teste de "Testar e adicionar" nunca é barrado nem deixa marca, nem com marca antiga gravada', () => {
    const dados = new Map([['hangar_servidores_desligados',
      JSON.stringify({ [SERVIDOR_CANDIDATO]: { failures: 9, retryAt: Date.now() + 1_800_000 } })]]);
    definirArmazem({ getItem: (k) => dados.get(k) ?? null, setItem: (k, v) => { dados.set(k, v); }, removeItem: (k) => { dados.delete(k); } });
    try {
      expect(retryAfterMs(SERVIDOR_CANDIDATO)).toBe(0);
      expect(dados.has('hangar_servidores_desligados')).toBe(false);
      registrarFalha(SERVIDOR_CANDIDATO);
      expect(estaDesligado(SERVIDOR_CANDIDATO)).toBe(false);
    } finally {
      definirArmazem(null);
    }
  });

  it('uma falha de rede já marca; ninguém mais procura por ele', () => {
    expect(estaDesligado('pc')).toBe(false);
    registrarFalha('pc');
    expect(estaDesligado('pc')).toBe(true);
  });

  it('ação explícita permite tentar antes do prazo', () => {
    registrarFalha('pc');
    retentarAgora('pc');
    expect(estaDesligado('pc')).toBe(false);
    // Continuou morta: uma falha e já desliga de novo.
    registrarFalha('pc');
    expect(estaDesligado('pc')).toBe(true);
    retentarAgora();                 // "buscar agora" sem id libera todos
    expect(estaDesligado('pc')).toBe(false);
  });

  it('prazo cresce até trinta minutos, sem confundir prazo vencido com resposta', () => {
    vi.useFakeTimers();
    for (const delay of [2000, 5000, 30000, 60000, 120000, 240000, 300000, 600000, 1800000, 1800000]) {
      registrarFalha('pc');
      expect(retryAfterMs('pc')).toBe(delay);
      vi.advanceTimersByTime(1000);
      registrarFalha('pc');
      expect(retryAfterMs('pc')).toBe(delay - 1000);
      vi.advanceTimersByTime(delay - 1000);
      expect(retryAfterMs('pc')).toBe(0);
      expect(estaDesligado('pc')).toBe(true);
    }
    registrarSucesso('pc');
    registrarFalha('pc');
    expect(retryAfterMs('pc')).toBe(2000);
  });

  it('quem respondeu nas últimas 24 h para em 30 s; passado isso, a escala volta', () => {
    vi.useFakeTimers();
    registrarSucesso('pc');
    for (const delay of [2000, 5000, 30000, 30000, 30000]) {
      registrarFalha('pc');
      expect(retryAfterMs('pc')).toBe(delay);
      vi.advanceTimersByTime(delay);
    }
    vi.advanceTimersByTime(24 * 60 * 60_000);
    registrarFalha('pc');
    expect(retryAfterMs('pc')).toBe(60000);
  });

  it('a lembrança de quem respondeu sobrevive ao recarregamento', () => {
    const guardado = new Map<string, string>();
    definirArmazem({
      getItem: (k: string) => guardado.get(k) ?? null,
      setItem: (k: string, v: string) => void guardado.set(k, v),
      removeItem: (k: string) => void guardado.delete(k),
    });
    try {
      registrarSucesso('pc');
      const salvo = guardado.get('hangar_servidores_responderam')!;
      _limparEsfriamentoParaTestes();
      guardado.set('hangar_servidores_responderam', salvo);
      expect(respondeuRecentemente('pc')).toBe(true);
      expect(respondeuRecentemente('outro')).toBe(false);
    } finally {
      definirArmazem(null);
    }
  });

  it('respondeu: sai da lista', () => {
    registrarFalha('pc');
    registrarSucesso('pc');
    expect(estaDesligado('pc')).toBe(false);
  });

  it('avisa a recuperação uma vez, depois de limpar a marca, e permite cancelar a assinatura', () => {
    const recovered = vi.fn((id: string) => expect(estaDesligado(id)).toBe(false));
    const off = onServerRecovered(recovered);
    try {
      registrarFalha('pc');
      registrarSucesso('pc');
      registrarSucesso('pc');
      expect(recovered).toHaveBeenCalledExactlyOnceWith('pc');
    } finally {
      off();
    }
    registrarFalha('pc');
    registrarSucesso('pc');
    expect(recovered).toHaveBeenCalledTimes(1);
  });

  it('cada servidor é independente e sair da lista apaga o estado', () => {
    registrarFalha('a');
    expect(estaDesligado('a')).toBe(true);
    expect(estaDesligado('b')).toBe(false);
    esquecerServidor('a');
    expect(estaDesligado('a')).toBe(false);
  });

  it('marca feita antes de o armazém chegar é gravada na injeção', () => {
    const guardado = new Map<string, string>();
    registrarFalha('pc');   // ainda sem armazém
    definirArmazem({
      getItem: (k: string) => guardado.get(k) ?? null,
      setItem: (k: string, v: string) => void guardado.set(k, v),
      removeItem: (k: string) => void guardado.delete(k),
    });
    try {
      expect(guardado.get('hangar_servidores_desligados')).toContain('pc');
      expect(estaDesligado('pc')).toBe(true);
    } finally {
      definirArmazem(null);
    }
  });

  it('a marca sobrevive ao recarregamento do app', () => {
    // O iOS descarrega e recarrega o PWA sozinho; com o estado só em memória, cada retomada
    // recomeçava a varredura — foi o que impediu as tentativas de chegarem a zero.
    const guardado = new Map<string, string>();
    // O core não toca `localStorage`: quem o tem (o web) entrega por `definirArmazem`.
    const falso = {
      getItem: (k: string) => guardado.get(k) ?? null,
      setItem: (k: string, v: string) => void guardado.set(k, v),
      removeItem: (k: string) => void guardado.delete(k),
    };
    definirArmazem(falso);
    try {
      registrarFalha('pc');
      expect(guardado.get('hangar_servidores_desligados')).toContain('pc');
      const saved = guardado.get('hangar_servidores_desligados')!;
      vi.useFakeTimers();
      vi.advanceTimersByTime(1500);
      _limparEsfriamentoParaTestes();          // simula o app subindo de novo…
      guardado.set('hangar_servidores_desligados', saved);
      expect(estaDesligado('pc')).toBe(true);  // …e a marca continua lá
      expect(retryAfterMs('pc')).toBeGreaterThan(400);
      expect(retryAfterMs('pc')).toBeLessThanOrEqual(500);
    } finally {
      definirArmazem(null);
    }
  });
});

describe('início da queda', () => {
  const armazemDe = (dados: Map<string, string>) => ({ getItem: (k: string) => dados.get(k) ?? null,
    setItem: (k: string, v: string) => { dados.set(k, v); }, removeItem: (k: string) => { dados.delete(k); } });

  it('conta da primeira falha e sobrevive à recarga até uma lista boa', async () => {
    vi.useFakeTimers({ now: 10_000 });
    const armazem = armazemDe(new Map());
    definirArmazem(armazem);
    try {
      expect(startOutage('pc')).toBe(10_000);
      vi.advanceTimersByTime(60_000);
      expect(startOutage('pc')).toBe(10_000);
      vi.resetModules();   // recarga: memória nova, mesmo armazém
      const recarregado = await import('./esfriamento');
      recarregado.definirArmazem(armazem);
      expect(recarregado.startOutage('pc')).toBe(10_000);
      expect(recarregado.endOutage('pc')).toBe(10_000);
      expect(recarregado.endOutage('pc')).toBeUndefined();
      expect(recarregado.startOutage('pc')).toBe(70_000);
      recarregado.definirArmazem(null);
    } finally {
      definirArmazem(null);
    }
  });

  it('não depende do prazo: protegido, resposta e "buscar agora" não reabrem a janela', () => {
    vi.useFakeTimers({ now: 10_000 });
    definirProtegido((id) => id === 'ativo');
    expect(startOutage('ativo')).toBe(10_000);
    registrarFalha('ativo');
    vi.advanceTimersByTime(5_000);
    registrarSucesso('ativo');   // ping ou erro HTTP: respondeu, mas a lista não veio
    retentarAgora('ativo');
    retentarAgora();
    expect(startOutage('ativo')).toBe(10_000);
  });

  it('servidor esquecido perde o início; o candidato nunca grava', () => {
    vi.useFakeTimers({ now: 10_000 });
    const dados = new Map<string, string>();
    definirArmazem(armazemDe(dados));
    try {
      startOutage('pc');
      startOutage(SERVIDOR_CANDIDATO);
      expect(JSON.parse(dados.get('hangar_quedas')!)).toEqual({ pc: 10_000 });
      vi.advanceTimersByTime(1_000);
      esquecerServidor('pc');
      expect(dados.has('hangar_quedas')).toBe(false);
      expect(startOutage('pc')).toBe(11_000);
    } finally {
      definirArmazem(null);
    }
  });
});
