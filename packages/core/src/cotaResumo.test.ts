import { describe, it, expect, beforeEach } from 'vitest';
import { contaComFolga, cotaDaConta, motivoCotaAntiga, resumoCota, type CotaContaResumo } from './cotaResumo';
import { overwriteGetLocale } from './paraglide/runtime';
import * as m from './paraglide/messages';

describe('cota antiga com a falha nova', () => {
  beforeEach(() => overwriteGetLocale(() => 'pt'));
  const lida = (motivo: string | null): CotaContaResumo =>
    ({ id: 'chave:a', estado: 'lida', janelas: [{ rotulo: '5h', pct: 40 }], motivo });
  const aviso = (causa: string) => m.cota_nao_atualizou({ causa });
  it('diz por que os números não atualizaram', () => {
    expect(motivoCotaAntiga(lida('sem-resposta'))).toBe(aviso(m.cota_causa_sem_resposta()));
    expect(motivoCotaAntiga(lida('http-429'))).toBe(aviso(m.cota_causa_limite()));
    expect(motivoCotaAntiga(lida('http-401'))).toBe(aviso(m.cota_causa_recusou()));
    expect(motivoCotaAntiga(lida('http-403'))).toBe(aviso(m.cota_causa_recusou()));
    expect(motivoCotaAntiga(lida('http-502'))).toBe(aviso(m.cota_causa_http({ status: '502' })));
    // 2xx com corpo que não serve não é erro da fonte: é formato.
    expect(motivoCotaAntiga(lida('http-200'))).toBe(aviso(m.cota_causa_formato()));
    expect(motivoCotaAntiga(lida('formato-desconhecido'))).toBe(aviso(m.cota_causa_formato()));
    expect(motivoCotaAntiga(lida('painel-sem-numeros'))).toBe(aviso(m.cota_causa_formato()));
    expect(motivoCotaAntiga(lida('erro-leitor'))).toBe(aviso(m.cota_causa_formato()));
    expect(motivoCotaAntiga(lida('resposta-grande'))).toBe(aviso(m.cota_causa_grande()));
    expect(motivoCotaAntiga(lida('credencial-invalida'))).toBe(aviso(m.cota_causa_credencial()));
    expect(motivoCotaAntiga(lida('credencial-ilegivel'))).toBe(aviso(m.cota_causa_credencial()));
    expect(motivoCotaAntiga(lida('causa-nova'))).toBe(aviso(m.cota_causa_outra()));
  });
  it('sem falha nova, ou fora da leitura boa, não há aviso', () => {
    expect(motivoCotaAntiga(lida(null))).toBeNull();
    // Conta ocupada (login, renovação, redefinição em curso) não é leitura que falhou.
    expect(motivoCotaAntiga(lida('sessao-viva'))).toBeNull();
    // A web manda a cota da linha sem `id`.
    expect(motivoCotaAntiga({ estado: 'lida', motivo: 'sem-resposta' })).toBe(aviso(m.cota_causa_sem_resposta()));
    expect(motivoCotaAntiga({ ...lida('sem-resposta'), estado: 'indisponivel' })).toBeNull();
    expect(motivoCotaAntiga(undefined)).toBeNull();
  });
});

describe('contaComFolga', () => {
  const cota = (path: string, janelas: CotaContaResumo['janelas'], estado: CotaContaResumo['estado'] = 'lida'): CotaContaResumo =>
    ({ id: `claude:${path}`, estado, janelas });
  const cheia = [{ rotulo: '5h', pct: 100, reset_ts: 9_999_999_999 }];
  const usada = (path: string, pct: number) => cota(path, [{ rotulo: '7d', pct }]);
  const contas = (...paths: string[]) => paths.map((path) => ({ path }));
  it('a 95% ou mais vai para a de mais folga, contando todas as janelas; sem leitura não entra', () => {
    const cotas = [cota('/a', cheia), cota('/b', [{ rotulo: '5h', pct: 70 }, { rotulo: '7d', pct: 10 }]),
      cota('/c', [{ rotulo: '5h', pct: 20 }, { rotulo: '7d opus', pct: 40, por_modelo: true }])];
    expect(contaComFolga('/a', [{ path: '/a', active: true }, { path: '/b' }, { path: '/c' }, { path: '/d' }], cotas)).toBe('/c');
    expect(contaComFolga('/a', contas('/a', '/b'), [usada('/a', 97), usada('/b', 40)])).toBe('/b');
    // Janela por modelo conta, como no servidor.
    expect(contaComFolga('/a', contas('/a', '/c'), [cota('/a', [{ rotulo: '7d opus', pct: 100, por_modelo: true }]), usada('/c', 20)])).toBe('/c');
    // A última leitura guardada de conta expirada conta, como no servidor.
    expect(contaComFolga('/a', contas('/a', '/b'), [cota('/a', cheia, 'expirada'), usada('/b', 10)])).toBe('/b');
    // Mas não serve de destino.
    expect(contaComFolga('/a', contas('/a', '/b'), [usada('/a', 97), cota('/b', [{ rotulo: '7d', pct: 10 }], 'expirada')])).toBeNull();
  });
  it('todas a 95% ou mais: a de mais folga, se tiver mais que a atual', () => {
    expect(contaComFolga('/a', contas('/a', '/b', '/c'), [usada('/a', 99), usada('/b', 98), usada('/c', 96)])).toBe('/c');
  });
  it('fica onde está: abaixo de 95%, nenhuma melhor ou sem conta', () => {
    expect(contaComFolga('/a', contas('/a', '/c'), [usada('/a', 94), usada('/c', 10)])).toBeNull();
    expect(contaComFolga('/a', contas('/a', '/b', '/c'), [usada('/a', 96), usada('/b', 100), cota('/c', cheia)])).toBeNull();
    expect(contaComFolga('/a', contas('/a', '/b'), [cota('/a', cheia), cota('/b', cheia)])).toBeNull();
    expect(contaComFolga(null, contas('/a', '/c'), [cota('/a', cheia), usada('/c', 20)])).toBeNull();
    // Janela que já virou não conta: os 100% guardados são de antes do reset.
    expect(contaComFolga('/a', contas('/a', '/c'), [cota('/a', [{ rotulo: '5h', pct: 100, reset_ts: 500 }]), usada('/c', 20)], 1000)).toBeNull();
  });
  it('empate fica com a ativa, em qualquer ordem', () => {
    const cotas = [cota('/a', cheia), cota('/c', [{ rotulo: '5h', pct: 20 }]), cota('/e', [{ rotulo: '5h', pct: 20 }])];
    expect(contaComFolga('/a', [{ path: '/a' }, { path: '/c', active: true }, { path: '/e' }], cotas)).toBe('/c');
    expect(contaComFolga('/a', [{ path: '/a' }, { path: '/e' }, { path: '/c', active: true }], cotas)).toBe('/c');
  });
});

const lida: CotaContaResumo = {
  id: 'claude:/home/x/.claude',
  estado: 'lida',
  janelas: [{ rotulo: '5h', pct: 42.4 }, { rotulo: '7d', pct: 17.6 }, { rotulo: 'Fable', pct: 100, por_modelo: true }],
};

describe('cotaDaConta', () => {
  it('casa pelo config dir com o prefixo claude:', () => {
    expect(cotaDaConta([lida], '/home/x/.claude')).toBe(lida);
    expect(cotaDaConta([lida], '/home/x/.claude-b')).toBeUndefined();
  });
});

describe('resumoCota', () => {
  it('uma janela por trecho, arredondada', () => {
    expect(resumoCota(lida)).toBe('5h 42% · 7d 18% · Fable 100%');
  });
  it('vazio sem leitura, sem conta ou sem janela', () => {
    expect(resumoCota(undefined)).toBe('');
    expect(resumoCota({ ...lida, estado: 'expirada' })).toBe('');
    expect(resumoCota({ ...lida, janelas: [] })).toBe('');
    expect(resumoCota({ ...lida, janelas: [{ rotulo: '5h', pct: NaN }] })).toBe('');
  });
});
