import { describe, expect, it } from 'vitest';
import { duracaoLegivel } from './orq';

describe('duracaoLegivel', () => {
	it('horas e minutos', () => {
		expect(duracaoLegivel('2026-08-22T09:00:00-03:00', '2026-08-23T23:30:00-03:00')).toBe('38h30');
	});
	it('só minutos', () => {
		expect(duracaoLegivel('2026-08-22T09:00:00-03:00', '2026-08-22T09:12:00-03:00')).toBe('12min');
	});
	it('59m40s arredonda pra 1h00, não 60min', () => {
		expect(duracaoLegivel('2026-08-22T09:00:00-03:00', '2026-08-22T09:59:40-03:00')).toBe('1h00');
	});
	it('sem fim → vazio', () => {
		expect(duracaoLegivel('2026-08-22T09:00:00-03:00', null)).toBe('');
	});
});
