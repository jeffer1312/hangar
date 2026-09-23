<script lang="ts">
  // Ligar esta máquina em outra: endereço e token em campos separados, o endereço normalizado
  // (IP, nome ou link inteiro), e um GET no candidato ANTES de gravar — nada toca o storage até o
  // servidor responder. O reload no sucesso é deliberado: a lista nova muda ativo/token e o SSE do
  // store precisa renascer limpo.
  import * as m from '../../paraglide/messages';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import QrScanner from '../QrScanner.svelte';
  import { normalizarEndereco } from '../../lib/url';
  import { getConfigForServer, registrarSucesso, SERVIDOR_CANDIDATO } from '@hangar/core';
  import { addServer } from '../../lib/auth';
  import { getIdentificador, type MaquinaDescoberta } from '../../lib/peers';
  import { registrarPeerDoisLados } from '../../lib/registrarPeerDoisLados';
  import type { Server } from '../../lib/auth';

  interface Busca {
    itens: MaquinaDescoberta[] | null;   // null = ainda não buscou
    buscando: boolean;
    erro: string;
    podeBuscar: boolean;
    onBuscar: () => void;
  }
  interface Props {
    fallbackFocus?: HTMLElement | null;
    onFechar: () => void;
    onAdicionada?: () => void;
    apiTarget?: Server | null;
    podeFalar?: boolean;
    enderecoInicial?: string;
    busca?: Busca;
  }
  let { fallbackFocus = null, onFechar, onAdicionada, apiTarget = null, podeFalar = false, enderecoInicial = '', busca }: Props = $props();
  let tokenEl = $state<HTMLInputElement | null>(null);

  function usarAchado(d: MaquinaDescoberta) {
    endereco = d.base_url;
    token = '';   // o token digitado era de outro servidor
    erro = '';
    tokenEl?.focus();
  }

  let endereco = $state(enderecoInicial);
  let token = $state('');
  let erro = $state('');
  let ocupado = $state(false);
  let scanning = $state(false);
  let enderecoEl = $state<HTMLInputElement | null>(null);
  let falar = $state(false);   // é uma pergunta: quem quer, marca
  // Ligado por padrão: é o que "adicionar servidor" sempre fez. Só é escolha quando há o outro
  // lado (recado) pra ficar no lugar — máquina só pra recado, sem as sessões dela na lista.
  let acompanhar = $state(true);

  // Link de pareamento colado inteiro: o token vai para o campo dele e o endereço fica só a origem.
  // Roda no blur, não no input — normalizar a cada tecla reescreveria o que a pessoa está digitando.
  function separarToken() {
    const n = normalizarEndereco(endereco);
    if (n?.token) { token = n.token; endereco = n.base; }
  }

  // Responde? Grava. Nome com ponto veio como https por dedução; se ninguém atendeu lá, o mesmo
  // nome em http na porta padrão é a segunda e última tentativa (FQDN de rede local) — só quando a
  // 1ª falha foi de REDE: uma resposta HTTP (401 etc) já veio de alguém, tentar a alternativa
  // trocaria essa mensagem por um "fetch failed" da porta que ninguém abriu.
  async function testarEAdicionar() {
    if (ocupado) return;
    const n = normalizarEndereco(endereco);
    if (!n) {
      erro = /[?&]token=/.test(endereco) ? m.maquinas_add_erro_token() : m.maquinas_add_erro_endereco();
      return;
    }
    const tok = (n.token ?? token).trim();
    if (!tok || /\s/.test(tok)) { erro = m.maquinas_add_erro_token(); return; }
    const soRecado = podeFalar && falar && !acompanhar;
    if (podeFalar && !falar && !acompanhar) { erro = m.maquinas_add_erro_nenhum(); return; }
    ocupado = true;
    erro = '';
    let base = n.base;
    // 20 s, não os 8 s padrão: daqui o probe sai do CELULAR pra máquina, e pela Tailscale em relay
    // a primeira conexão passa de 8 s.
    const PRAZO_MS = 20000;
    try {
      await getConfigForServer({ id: SERVIDOR_CANDIDATO, label: base, baseUrl: base, token: tok }, PRAZO_MS);
    } catch (e) {
      const msg1 = e instanceof Error ? e.message : String(e);
      const respostaHttp = e instanceof Error && /^\d{3}:/.test(e.message);
      if (!n.alternativa || respostaHttp) {
        erro = e instanceof Error ? `${m.falha_conexao()}: ${msg1}` : m.erro_desconhecido();
        ocupado = false;
        return;
      }
      base = n.alternativa;
      try {
        await getConfigForServer({ id: SERVIDOR_CANDIDATO, label: base, baseUrl: base, token: tok }, PRAZO_MS);
      } catch (e2) {
        const msg2 = e2 instanceof Error ? e2.message : String(e2);
        erro = `${m.falha_conexao()}: ${msg1} · ${msg2}`;
        ocupado = false;
        return;
      }
    }
    // Fora do try/catch acima: erro daqui pra baixo não é do probe, e rotulá-lo de "falha na
    // conexão" mentiria sobre a causa.
    // Registrar o peer antes de gravar no navegador: o reload que vem a seguir apaga este componente,
    // e a lista mostra o estado do registro quando voltar. Falha aqui não é motivo para não acompanhar.
    if (podeFalar && falar) {
      try {
        const { identificador } = await getIdentificador({ id: SERVIDOR_CANDIDATO, label: base, baseUrl: base, token: tok });
        if (!identificador) throw new Error(m.maquinas_add_erro_sem_identificador());
        await registrarPeerDoisLados(apiTarget, { id: identificador, base_url: base, token: tok });
      } catch (e) {
        // Só recado: o registro ERA o pedido, e falha nele é o resultado — fica à vista.
        // Com acompanhar, a lista dirá "só uma das pontas responde" e o que a pessoa pediu segue.
        if (soRecado) { erro = e instanceof Error ? e.message : m.erro_desconhecido(); ocupado = false; return; }
      }
    }
    // Sem reload: `addServer` dispara o envio da lista pro hub de sincronização, e recarregar a
    // página matava esse envio no meio — ao voltar, o hub (sem a máquina nova) mandava e ela
    // sumia. Quem precisa reagir escuta `onServersChanged`; a tela de máquinas recarrega por
    // `onAdicionada`.
    // Endereço que já estava na lista marcado como desligado: o teste acabou de provar que responde.
    if (!soRecado) registrarSucesso(addServer(base, tok, undefined, { ativar: false }).id);
    ocupado = false;
    onAdicionada?.();
    onFechar();
  }

  function lerQr(texto: string) {
    scanning = false;
    endereco = texto.trim();
    separarToken();
    void testarEAdicionar();
  }

  // Fechar com o teste em voo é recusado: o diálogo é o único lugar onde o erro tardio aparece,
  // e desmontá-lo faria a falha morrer calada.
  function fechar() {
    if (!ocupado) onFechar();
  }

  const podeTestar = $derived(!ocupado && !!endereco.trim());
</script>

{#if scanning}
  <QrScanner onScan={lerQr} onClose={() => (scanning = false)} />
{:else}
  <ConfirmDialog title={m.maquinas_adicionar()} aria={m.maquinas_adicionar()} role="dialog" wide
    {fallbackFocus} initialFocus={enderecoEl}
    onClose={fechar}
    actions={[
      // Cancelar explícito: no celular o card cobre a tela quase inteira e não sobra fundo pra tocar.
      { label: m.comum_cancelar(), disabled: ocupado, onClick: fechar },
      { label: m.sessao_escanear_qr(), disabled: ocupado, onClick: () => (scanning = true) },
      { label: m.maquinas_add_testar(), kind: 'primary', disabled: !podeTestar, onClick: testarEAdicionar },
    ]}>
    {#if busca}
      <!-- Sob demanda: cada busca bate em todos os peers online, então não roda ao abrir. -->
      <div class="am-busca">
        <div class="am-busca-cab">
          <span class="am-busca-txt">{busca.podeBuscar ? m.maquinas_buscar_ajuda() : m.maquinas_buscar_sem_maquina()}</span>
          <button type="button" class="am-btn" onclick={busca.onBuscar} disabled={busca.buscando || !busca.podeBuscar || ocupado}>
            {busca.buscando ? m.maquinas_buscando() : m.maquinas_buscar_tailscale()}
          </button>
        </div>
        {#if busca.erro}<p class="am-erro" role="status">{busca.erro}</p>{/if}
        {#if busca.itens !== null && !busca.buscando}
          {#if busca.itens.length === 0}
            <p class="am-ajuda" role="status">{m.maquinas_buscar_nada()}</p>
          {:else}
            <p class="am-ajuda">{m.maquinas_buscar_achou()}</p>
            <ul class="am-achados">
              {#each busca.itens as d (d.base_url)}
                <li class="am-achado">
                  <span class="am-achado-txt"><span>{d.nome}</span><span class="am-achado-url">{d.base_url}</span></span>
                  <button type="button" class="am-btn" onclick={() => usarAchado(d)} disabled={ocupado}>{m.maquinas_adicionar()}</button>
                </li>
              {/each}
            </ul>
          {/if}
        {/if}
      </div>
    {/if}
    <label class="am-campo">
      <span class="am-rot">{m.maquinas_add_endereco()}</span>
      <input class="am-input" bind:this={enderecoEl} bind:value={endereco}
             aria-label={m.maquinas_add_endereco()}
             aria-invalid={!!erro} aria-describedby={erro ? 'am-erro' : undefined}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             disabled={ocupado}
             onblur={separarToken}
             oninput={() => (erro = '')}
             onkeydown={(e) => { if (e.key === 'Enter') { separarToken(); void testarEAdicionar(); } }} />
      <span class="am-ajuda">{m.maquinas_add_endereco_ajuda()}</span>
    </label>
    <label class="am-campo">
      <span class="am-rot">{m.sessao_token()}</span>
      <input class="am-input" bind:this={tokenEl} bind:value={token}
             aria-label={m.sessao_token()}
             aria-invalid={!!erro} aria-describedby={erro ? 'am-erro' : undefined}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             disabled={ocupado}
             oninput={() => (erro = '')}
             onkeydown={(e) => { if (e.key === 'Enter') void testarEAdicionar(); }} />
      <span class="am-ajuda">{m.maquinas_add_token_ajuda({ variavel: 'CP_AUTH_TOKEN' })}</span>
    </label>
    {#if podeFalar}
      <label class="am-falar-linha">
        <input class="switch am-acompanhar" type="checkbox" bind:checked={acompanhar} disabled={ocupado} onchange={() => (erro = '')} />
        <span class="am-falar-txt">
          <span>{m.maquinas_add_acompanhar()}</span>
          <span class="am-ajuda">{m.maquinas_add_acompanhar_ajuda()}</span>
        </span>
      </label>
      <label class="am-falar-linha">
        <input class="switch am-falar" type="checkbox" bind:checked={falar} disabled={ocupado} onchange={() => (erro = '')} />
        <span class="am-falar-txt">
          <span>{m.maquinas_add_falar()}</span>
          <span class="am-ajuda">{m.maquinas_add_falar_ajuda()}</span>
        </span>
      </label>
    {/if}
    {#if ocupado}<p class="am-status" aria-live="polite">{m.maquinas_add_testando()}</p>{/if}
    {#if erro}<p class="am-erro" id="am-erro" role="alert">{erro}</p>{/if}
  </ConfirmDialog>
{/if}

<style>
  .am-busca { display: flex; flex-direction: column; gap: var(--space-2); margin-bottom: var(--space-4); padding-bottom: var(--space-3); border-bottom: 1px solid var(--border-subtle); }
  .am-busca-cab { display: flex; align-items: center; gap: var(--space-3); }
  .am-busca-txt { flex: 1; font-size: 0.8rem; color: var(--text-muted); line-height: 1.4; }
  .am-btn { flex-shrink: 0; min-height: 36px; padding: 0 var(--space-3); border-radius: var(--radius-sm); border: 1px solid var(--border-default); color: var(--text-primary); font-size: 0.85rem; }
  .am-btn:disabled { opacity: 0.5; }
  .am-achados { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; }
  .am-achado { display: flex; align-items: center; gap: var(--space-3); padding: var(--space-2) var(--space-3); }
  .am-achado + .am-achado { border-top: 1px solid var(--border-subtle); }
  .am-achado-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; font-size: 0.9rem; }
  .am-achado-url { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); word-break: break-all; }
  .am-campo { display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3); }
  .am-rot { font-size: 0.85rem; color: var(--text-muted); }
  .am-input {
    background: var(--surface-inset);
    border: 1px solid var(--border-default);
    border-radius: var(--radius-sm);
    color: var(--text-primary);
    padding: var(--space-2) var(--space-3);
    font: inherit;
  }
  .am-ajuda { font-size: 0.8rem; color: var(--text-muted); }
  .am-falar-linha { display: flex; align-items: center; gap: var(--space-3); margin-bottom: var(--space-3); }
  .am-falar-txt { display: flex; flex-direction: column; gap: 2px; }
  .am-status { font-size: 0.85rem; color: var(--text-muted); margin: 0; }
  .am-erro { color: var(--error); font-size: 0.85rem; margin: 0; }
</style>
