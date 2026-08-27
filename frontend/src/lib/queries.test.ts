import { beforeEach, describe, expect, it, vi } from 'vitest';

let ativo: string | null = 'srv-a';
vi.mock('./auth', () => ({ getActiveId: () => ativo }));
vi.mock('./api', () => ({ getOrqPolitica: vi.fn(), getOrqGrupo: vi.fn(), getOrqDetalheForServer: vi.fn() }));
vi.mock('./credenciais', () => ({ listarCredenciais: vi.fn() }));

const { credenciais, orqDetalhe, orqGrupo, orqPolitica } = await import('./queries');

// O servidor ativo muda debaixo da tela (apiFetch resolve baseUrl na hora). Chave sem ele serviria
// a política/o grupo da máquina anterior — dado errado, sem nenhum sinal de que está errado.
describe('chave das queries de orquestração', () => {
	beforeEach(() => { ativo = 'srv-a'; });

	it('separa por servidor', () => {
		const a = orqPolitica().queryKey;
		ativo = 'srv-b';
		expect(orqPolitica().queryKey).not.toEqual(a);
	});

	it('separa por sessão', () => {
		expect(orqGrupo('uma').queryKey).not.toEqual(orqGrupo('outra').queryKey);
	});

	it('sem servidor ativo não colide com um servidor chamado vazio', () => {
		ativo = null;
		expect(orqPolitica().queryKey).toContain('-');
	});

	// Alvo null = "servidor ativo". Se a chave guardasse um literal em vez do id de quem está
	// ativo, trocar de máquina e reabrir Contas dentro do staleTime mostrava as contas da anterior.
	it('credenciais do alvo implícito seguem o servidor ativo', () => {
		const a = credenciais(null).queryKey;
		ativo = 'srv-b';
		expect(credenciais(null).queryKey).not.toEqual(a);
	});

	// Execução lida enquanto viva e reaberta já terminada: mesma chave + staleTime Infinity
	// congelaria o snapshot do meio da execução, sem a última rodada nem o veredito.
	it('detalhe de execução viva e terminada não dividem a mesma entrada', () => {
		const s = { id: 'srv-a' } as never;
		expect(orqDetalhe(s, 'exec-1', false).queryKey).not.toEqual(orqDetalhe(s, 'exec-1', true).queryKey);
	});
});
