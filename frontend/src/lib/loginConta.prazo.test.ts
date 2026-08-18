// @vitest-environment node
// Teto de tempo do loginConta no caminho EXPLÍCITO. ESTE ARQUIVO É `node` DE PROPÓSITO: o
// fetch do happy-dom IGNORA AbortSignal.timeout (medido em 18/08/2026 nesta árvore — um teto de
// 2s não cortou um pedido de 12s), então nenhum teste em happy-dom pode provar ou desmentir
// prazo. Só o fetch do node (e do navegador) honra o sinal. Não "padronizar" para happy-dom:
// o teste pararia de provar qualquer coisa, calado.
//
// Por que os tetos existem: o backend segura /login/codigo em laço até _TIMEOUT_S=300s esperando
// o OAuth propagar — o cliente do alvo explícito não pode desistir em 8s de uma chamada que o
// servidor tem direito de segurar por 300s (bloqueador da rodada 1). As rotas rápidas (iniciar/
// passo/cancelar) seguem com 8s: servidor atrás de VPN não recusa conexão, pendura — sem teto o
// login ficava "aguardando" pra sempre.
import { describe, it, expect, beforeAll, afterAll } from 'vitest';
import { createServer, type Server as HttpServer } from 'node:http';
import type { AddressInfo } from 'node:net';
import type { Server } from './auth';
import * as m from '../paraglide/messages';

// auth.ts toca localStorage no load (migrate()) e node nao tem — stub minimo ANTES do import
// dinamico (mesmo padrao de auth.test.ts/api.test.ts); migrate() so faz getItem -> null, sai cedo.
const store = new Map<string, string>();
(globalThis as any).localStorage = {
  getItem: (k: string) => (store.has(k) ? store.get(k)! : null),
  setItem: (k: string, v: string) => store.set(k, String(v)),
  removeItem: (k: string) => store.delete(k),
};

const { iniciarLogin, confirmarLogin } = await import('./loginConta');

// baseUrl montado DEPOIS do listen: porta 0 = o kernel escolhe uma livre (R1 da rodada 2) — porta
// fixa ocupada derrubaria o arquivo com EADDRINUSE e a falha leria como "o teto quebrou".
const SRV_BASE: Omit<Server, 'baseUrl'> = { id: 'srv-b', label: 'B', token: 't-b' };
let SRV: Server;

// Servidor de mentira: responde /login/codigo DEPOIS de um atraso configurável (dentro dos 300s
// que o backend aceita); o resto na hora.
let srv: HttpServer;
let atrasoCodigoMs = 0;
let mudo = false;   // true = nao responde nada (servidor pendurado atras de VPN)
const caminhos: string[] = [];

beforeAll(async () => {
  srv = createServer((req, res) => {
    const u = new URL(req.url ?? '/', 'http://127.0.0.1');
    caminhos.push(u.pathname);
    if (mudo) return;   // pendura: o socket fica aberto ate o cliente desistir
    if (u.pathname.endsWith('/login/codigo')) {
      setTimeout(() => {
        res.writeHead(200, { 'Content-Type': 'application/json' });
        res.end(JSON.stringify({ ok: true, email: 'u@x.com', plano: 'max' }));
      }, atrasoCodigoMs);
      return;
    }
    res.writeHead(200, { 'Content-Type': 'application/json' });
    res.end('{"ok":true}');
  });
  await new Promise<void>((r) => srv.listen(0, '127.0.0.1', r));
  const addr = srv.address() as AddressInfo;
  SRV = { ...SRV_BASE, baseUrl: `http://127.0.0.1:${addr.port}` };
});
afterAll(async () => {
  await new Promise<void>((r) => srv.close(() => r()));
});

describe('loginConta — tetos do caminho explícito', () => {
  it('confirmarLogin no alvo explícito ESPERA a resposta longa do backend (9s < 310s) e conclui',
    { timeout: 30_000 }, async () => {
    atrasoCodigoMs = 9_000;
    const t0 = Date.now();
    const r = await confirmarLogin(SRV, 'testes', 'CODE-123');
    const dur = Date.now() - t0;
    expect(r).toEqual({ ok: true, email: 'u@x.com', plano: 'max' });
    expect(dur).toBeGreaterThanOrEqual(8_500);   // esperou de verdade, não devolveu antes
    expect(dur).toBeLessThan(20_000);            // e não estourou em 8s como o defeito fazia
    expect(caminhos.some((p) => p.endsWith('/login/codigo'))).toBe(true);
  });

  it('rota rápida (iniciar) com servidor mudo estoura o teto de 8s e devolve a frase TRADUZIDA',
    { timeout: 30_000 }, async () => {
    // O servidor não responde o /login: o teto de 8s do caminho explícito corta. O erro que
    // chega à tela tem de ser a frase da casa ("o login demorou demais"), nunca o 'signal
    // timed out' cru do navegador — o ramo que traduzia era código morto (checava a RESPOSTA,
    // mas o teto lança de dentro do fetch).
    atrasoCodigoMs = 0;
    mudo = true;
    const t0 = Date.now();
    await expect(iniciarLogin(SRV, 'testes')).rejects.toThrow(m.erro_login_timeout());
    expect(Date.now() - t0).toBeGreaterThanOrEqual(7_500);
    mudo = false;
  });
});
