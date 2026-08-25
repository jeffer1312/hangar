import { describe, expect, it } from 'vitest';
import { etapaAtual } from './orqAgora';

describe('etapaAtual', () => {
	it('sem entrega → execução', () => {
		expect(etapaAtual([{ tipo: 'task_inicio' }])).toBe('execucao');
	});
	it('entrega sem veredito → revisão', () => {
		expect(etapaAtual([{ tipo: 'task_inicio' }, { tipo: 'entrega' }])).toBe('revisao');
	});
	it('devolvido → volta pra execução', () => {
		expect(etapaAtual([{ tipo: 'entrega' }, { tipo: 'veredito', resultado: 'devolvido' }])).toBe('execucao');
	});
	it('aprova → aprovada', () => {
		expect(etapaAtual([{ tipo: 'entrega' }, { tipo: 'veredito', resultado: 'aprova' }])).toBe('aprovada');
	});
});
