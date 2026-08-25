export function duracaoLegivel(inicioIso: string | null, fimIso: string | null): string {
	if (!inicioIso || !fimIso) return '';
	const ms = new Date(fimIso).getTime() - new Date(inicioIso).getTime();
	if (!Number.isFinite(ms) || ms < 0) return '';
	const totalMin = Math.round(ms / 60_000); // arredonda UMA vez; 59m40s vira 1h00, nunca 60min
	const h = Math.floor(totalMin / 60);
	const m = totalMin % 60;
	return h ? `${h}h${String(m).padStart(2, '0')}` : `${m}min`;
}
