import { beforeEach, describe, expect, it, vi } from 'vitest';

let ativo: string | null = 'srv-a';
vi.mock('./auth', () => ({ getActiveId: () => ativo }));
// O core traz muito mais que estas cinco funções; sem espalhar o original o mock derruba
// todo o resto que `queries.ts` importa de lá.
vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  getOrqPolitica: vi.fn(), getOrqGrupo: vi.fn(), getOrqDetalheForServer: vi.fn(),
  getEngines: vi.fn(), getEnginesForServer: vi.fn(),
}));
vi.mock('./credenciais', () => ({ listarCredenciais: vi.fn() }));

const { credenciais, motores, orqDetalhe, orqGrupo, orqPolitica,
        lerCaudaChat, guardarCaudaChat } = await import('./queries');

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

	// Mesmo defeito da `credenciais`: o alvo implícito é "o servidor ativo", e ele muda.
	it('motores do alvo implícito seguem o servidor ativo', () => {
		const a = motores(null).queryKey;
		ativo = 'srv-b';
		expect(motores(null).queryKey).not.toEqual(a);
		expect(motores({ id: 'srv-x' } as never).queryKey).toEqual(['motores', 'srv-x']);
	});

	// Execução lida enquanto viva e reaberta já terminada: mesma chave + staleTime Infinity
	// congelaria o snapshot do meio da execução, sem a última rodada nem o veredito.
	it('detalhe de execução viva e terminada não dividem a mesma entrada', () => {
		const s = { id: 'srv-a' } as never;
		expect(orqDetalhe(s, 'exec-1', false).queryKey).not.toEqual(orqDetalhe(s, 'exec-1', true).queryKey);
	});
});

// A cauda do chat é o que faz voltar pra uma sessão pintar sem rede, e o validador ao lado dela é
// o que dispensa regra de invalidação. Errar a chave aqui não dá erro: mostra a conversa de OUTRA
// sessão (ou de outra máquina) como se fosse a certa.
describe('cauda do chat entre aberturas', () => {
	beforeEach(() => { ativo = 'srv-a'; });

	const cauda = (n: number, etag: string | null = 'W1') => ({
		eventos: Array.from({ length: n }, (_, i) => ({ id: `e${i}` })) as never,
		etag,
	});

	it('guarda e devolve a mesma cauda, com o validador', () => {
		guardarCaudaChat('srv-a', 'sessao', cauda(3, 'abc'));
		expect(lerCaudaChat('srv-a', 'sessao')?.eventos).toHaveLength(3);
		expect(lerCaudaChat('srv-a', 'sessao')?.etag).toBe('abc');
	});

	// O offset do stream JÁ foi guardado aqui e causou regressão: restaurado, o SSE retomava à
	// frente do que o cache tinha e a conversa ficava parada na última mensagem enviada. O cache
	// carrega eventos e validador, nada mais — quem retoma o stream é o próprio stream.
	it('não guarda offset de stream junto', () => {
		guardarCaudaChat('srv-a', 'sessao', cauda(3));
		expect(Object.keys(lerCaudaChat('srv-a', 'sessao')!).sort()).toEqual(['etag', 'eventos']);
	});

	it('sessão sem cauda guardada devolve undefined, não a de outra', () => {
		guardarCaudaChat('srv-a', 'uma', cauda(2));
		expect(lerCaudaChat('srv-a', 'outra')).toBeUndefined();
	});

	// Mesmo nome de sessão em duas máquinas é comum (`hangar` local e `hangar` na VPS): sem o id do
	// servidor na chave, trocar de máquina serviria a conversa da anterior.
	it('separa por servidor', () => {
		guardarCaudaChat('srv-a', 'hangar', cauda(5));
		expect(lerCaudaChat('srv-b', 'hangar')).toBeUndefined();
		expect(lerCaudaChat('srv-a', 'hangar')?.eventos).toHaveLength(5);
	});

	// O servidor vem por ARGUMENTO justamente porque `idAtivo()` já mudou quando o Chat desmonta:
	// navegando pro chat de outra máquina, `applyRouteServer` troca o ativo ANTES do onDestroy.
	// Se a chave resolvesse o ativo na hora da escrita, a conversa de srv-a era gravada em srv-b —
	// e a próxima abertura da sessão homônima de srv-b pintava a conversa da outra máquina.
	it('gravar não depende de quem está ativo no momento da escrita', () => {
		guardarCaudaChat('srv-a', 'hangar', cauda(7));
		ativo = 'srv-b';                                     // o ativo mudou; a cauda é de srv-a
		expect(lerCaudaChat('srv-b', 'hangar')).toBeUndefined();
		expect(lerCaudaChat('srv-a', 'hangar')?.eventos).toHaveLength(7);
	});

	// O /clear grava cauda VAZIA (e sem validador) em vez de deixar a antiga: é o único ponto que
	// sabe que o transcript morreu, e sem isso a conversa apagada piscaria na próxima entrada — e o
	// etag do arquivo velho ainda seria oferecido ao servidor.
	it('cauda vazia sobrescreve a anterior, validador junto', () => {
		guardarCaudaChat('srv-a', 'sessao', cauda(4, 'velho'));
		guardarCaudaChat('srv-a', 'sessao', { eventos: [], etag: null });
		expect(lerCaudaChat('srv-a', 'sessao')?.eventos).toHaveLength(0);
		expect(lerCaudaChat('srv-a', 'sessao')?.etag).toBeNull();
	});
});

