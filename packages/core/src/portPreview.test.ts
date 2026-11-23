import { describe, expect, it, vi } from 'vitest';
import { openPortPreview, PortNotPageError } from './portPreview';
import type { Server } from './servers';

const server = { id: 's', label: 'casa', baseUrl: 'http://casa:8765', token: 't' } as Server;
const fail = (status: number, code?: string) => Object.assign(new Error(`${status}: x`), { status, code });

function deps(over: Partial<Parameters<typeof openPortPreview>[2] & object> = {}) {
  return {
    connect: vi.fn(async () => ({ url: 'https://p.connect/?l=1' })),
    tailscale: vi.fn(async () => ({ url: 'https://casa.ts.net:10000' })),
    ...over,
  };
}

describe('openPortPreview', () => {
  it('Connect ligado: usa o link dele e não toca no Tailscale', async () => {
    const d = deps();
    expect(await openPortPreview(server, 3000, d)).toEqual({ url: 'https://p.connect/?l=1', via: 'connect' });
    expect(d.tailscale).not.toHaveBeenCalled();
  });

  it('máquina sem Connect cai no Tailscale', async () => {
    const d = deps({ connect: vi.fn(async () => { throw fail(409, 'connect_not_linked'); }) });
    expect(await openPortPreview(server, 3000, d)).toEqual({ url: 'https://casa.ts.net:10000', via: 'tailscale' });
    expect(d.tailscale).toHaveBeenCalledWith(3000, server);
  });

  it('Connect fora do ar e Tailscale também: o erro mostrado é o do Connect', async () => {
    const down = fail(409, 'connect_not_running');
    const d = deps({ connect: vi.fn(async () => { throw down; }), tailscale: vi.fn(async () => { throw fail(503, undefined); }) });
    await expect(openPortPreview(server, 3000, d)).rejects.toBe(down);
  });

  it('porta do próprio Hangar recusada pelo Connect não tenta o Tailscale', async () => {
    const refused = fail(422, 'connect_preview_port_refused');
    const d = deps({ connect: vi.fn(async () => { throw refused; }) });
    await expect(openPortPreview(server, 8765, d)).rejects.toBe(refused);
    expect(d.tailscale).not.toHaveBeenCalled();
  });

  it('Postgres pelo Connect: o servidor diz que não é página e o Tailscale nem é tentado', async () => {
    const d = deps({ connect: vi.fn(async () => { throw fail(422, 'port_not_http'); }) });
    await expect(openPortPreview(server, 5432, d)).rejects.toBeInstanceOf(PortNotPageError);
    expect(d.tailscale).not.toHaveBeenCalled();
  });

  it('Postgres pelo Tailscale: a mesma recusa vira o aviso de que não é página', async () => {
    const d = deps({ connect: vi.fn(async () => { throw fail(404); }), tailscale: vi.fn(async () => { throw fail(422, 'port_not_http'); }) });
    await expect(openPortPreview(server, 5432, d)).rejects.toBeInstanceOf(PortNotPageError);
  });
});
