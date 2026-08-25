export type Etapa = 'execucao' | 'revisao' | 'portao' | 'aprovada';

// Etapa da task pelo ÚLTIMO acontecimento dela: entrega sem veredito = está com o revisor; veredito
// que não aprova devolve a bola pro executor. `portao` fica pro caso em que o app souber do merge —
// hoje nenhum evento o marca, e inventar etapa seria mentir na faixa.
export function etapaAtual(eventos: { tipo: string; resultado?: string }[]): Etapa {
	const ultimo = [...eventos].reverse().find((e) => e.tipo === 'entrega' || e.tipo === 'veredito');
	if (!ultimo) return 'execucao';
	if (ultimo.tipo === 'entrega') return 'revisao';
	return ultimo.resultado === 'aprova' ? 'aprovada' : 'execucao';
}
