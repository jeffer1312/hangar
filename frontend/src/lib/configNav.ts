// frontend/src/lib/configNav.ts
import { comConfig, type TelaConfig } from './configRoute';

// Navegar para o painel SEMPRE por aqui — menu da conta, lista lateral, drill-down do celular.
//
// `window.location.hash =` (nao pushState): ele empilha a entrada E dispara `hashchange`, o unico
// evento que o roteador escuta (App.svelte; nao ha um `popstate` no frontend inteiro).
//
// O replaceState logo depois NAO navega: so carimba a profundidade NA ENTRADA que acabou de nascer,
// pra ela viajar junto em back, forward e reload. DOIS argumentos: um terceiro `''` resolveria pra
// base SEM fragmento e apagaria o endereco que acabamos de montar.
export function abrirConfig(tela: TelaConfig, srv: string | null): void {
  const destino = comConfig(window.location.hash, tela, srv);
  // Hash igual nao empilha entrada nenhuma (tocar duas vezes no mesmo item da lista lateral). Sem
  // esta saida, o replaceState abaixo incrementaria a profundidade da entrada CORRENTE e o ✕ passaria
  // a pular pra tras da conversa.
  if (destino === window.location.hash) return;
  const prof = ((history.state?.cpDepth as number | undefined) ?? 0) + 1;
  window.location.hash = destino;
  history.replaceState({ ...history.state, cpDepth: prof }, '');
}

export function fecharConfig(): void {
  const prof = (history.state?.cpDepth as number | undefined) ?? 0;
  // go(-prof) desfaz TODAS as paradas que o painel empilhou, de uma vez: fechar volta pra tela de
  // tras, nao pra tela anterior do painel.
  if (prof > 0) history.go(-prof);
  // prof 0 so acontece em entrada que nao nasceu daqui: nao ha o que desempilhar, so limpar.
  else window.location.replace(comConfig(window.location.hash, null));
}
