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
  import { addServer, renameServer, setServerDisabled, updateServer } from '../../lib/auth';
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
    busca?: Busca;
    // Nome do servidor escolhido: é nele (e na máquina nova) que o recado grava.
    esteNome?: string;
    // Com a sincronização ligada a lista vale em todos os aparelhos da conta; sem ela, só aqui.
    sincronizada?: boolean;
    // Máquinas que este aparelho já tem, pelo identificador: o mesmo servidor por outro endereço
    // troca o endereço dele em vez de virar uma segunda linha.
    conhecidas?: { identificador: string; server: Server }[];
  }
  let { fallbackFocus = null, onFechar, onAdicionada, apiTarget = null, podeFalar = false, busca,
        esteNome = '', sincronizada = false, conhecidas = [] }: Props = $props();
  let tokenEl = $state<HTMLInputElement | null>(null);

  function usarAchado(d: MaquinaDescoberta) {
    endereco = d.base_url;
    token = '';   // o token digitado era de outro servidor
    erro = '';
    achado = null;
    tokenEl?.focus();
  }

  let endereco = $state('');
  let token = $state('');
  let erro = $state('');
  // Resultado do teste: quem respondeu nesse endereço. Adicionar só aparece depois dele, porque
  // o nome sugerido e a checagem de máquina repetida dependem do identificador que ela deu.
  let achado = $state<{ base: string; tok: string; identificador: string; idFalha: string } | null>(null);
  let nome = $state('');
  const repetida = $derived(achado?.identificador ? conhecidas.find((c) => c.identificador === achado!.identificador) ?? null : null);
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
  async function testar() {
    if (ocupado) return;
    const n = normalizarEndereco(endereco);
    if (!n) {
      erro = /[?&]token=/.test(endereco) ? m.maquinas_add_erro_token() : m.maquinas_add_erro_endereco();
      return;
    }
    const tok = (n.token ?? token).trim();
    if (!tok || /\s/.test(tok)) { erro = m.maquinas_add_erro_token(); return; }
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
    // Respondeu: pergunta quem é. Sem identificador ainda dá pra acompanhar; o nome sai do endereço.
    let identificador = '';
    // Falha aqui não é "sem identificador": sem o nome não dá pra saber se é máquina repetida, e a
    // tela diz isso em vez de oferecer uma linha nova calada.
    let idFalha = '';
    try {
      identificador = (await getIdentificador({ id: SERVIDOR_CANDIDATO, label: base, baseUrl: base, token: tok })).identificador ?? '';
    } catch (e) { identificador = ''; idFalha = e instanceof Error ? e.message : String(e); }
    achado = { base, tok, identificador, idFalha };
    nome = conhecidas.find((c) => c.identificador === identificador)?.server.label || identificador || hostDe(base);
    ocupado = false;
  }

  // IP fica inteiro: o primeiro pedaço de 192.168.0.10 seria "192".
  const hostDe = (u: string) => {
    try {
      const h = new URL(u).hostname;
      return /^[\d.]+$|:/.test(h) ? h : h.split('.')[0];
    } catch { return u; }
  };

  async function adicionar() {
    if (ocupado || !achado) return;
    const { base, tok, identificador } = achado;
    const rotulo = nome.trim() || identificador || hostDe(base);
    // Mesma máquina por outro endereço: troca o endereço (e o token) da entrada que já existe.
    if (repetida) {
      updateServer(repetida.server.id, { baseUrl: base, token: tok });
      // Adicionar de novo é pedir para ver: uma entrada desligada volta a mostrar as sessões.
      if (repetida.server.disabled) setServerDisabled(repetida.server.id, false);
      if (rotulo !== repetida.server.label) renameServer(repetida.server.id, rotulo);
      registrarSucesso(repetida.server.id);
      onAdicionada?.();
      onFechar();
      return;
    }
    const soRecado = podeFalar && falar && !acompanhar;
    if (podeFalar && !falar && !acompanhar) { erro = m.maquinas_add_erro_nenhum(); return; }
    ocupado = true;
    erro = '';
    // Registrar o peer antes de gravar no navegador. Falha aqui não é motivo para não acompanhar.
    if (podeFalar && falar) {
      try {
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
    if (!soRecado) registrarSucesso(addServer(base, tok, rotulo, { ativar: false }).id);
    ocupado = false;
    onAdicionada?.();
    onFechar();
  }

  function lerQr(texto: string) {
    scanning = false;
    endereco = texto.trim();
    separarToken();
    void testar();
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
  {@const titulo = sincronizada ? m.servidores_adicionar_lista() : m.servidores_adicionar_aparelho()}
  <ConfirmDialog title={titulo} aria={titulo} role="dialog" wide
    {fallbackFocus} initialFocus={enderecoEl}
    onClose={fechar}
    actions={[
      // Cancelar explícito: no celular o card cobre a tela quase inteira e não sobra fundo pra tocar.
      { label: m.comum_cancelar(), disabled: ocupado, onClick: fechar },
      { label: m.sessao_escanear_qr(), disabled: ocupado, onClick: () => (scanning = true) },
      achado
        ? { label: repetida ? m.servidores_add_usar_endereco() : m.servidores_add_adicionar(), kind: 'primary', disabled: ocupado, onClick: adicionar }
        : { label: m.servidores_add_testar(), kind: 'primary', disabled: !podeTestar, onClick: testar },
    ]}>
    <!-- Adicionar grava no APARELHO, não no servidor: sem esta faixa parecia que a máquina passava
         a aparecer para todo mundo que usa o servidor. -->
    <p class="am-faixa">{sincronizada ? m.servidores_add_faixa_sync() : m.servidores_add_faixa({ este: esteNome || '—' })}</p>
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
             oninput={() => { erro = ''; achado = null; }}
             onkeydown={(e) => { if (e.key === 'Enter') { separarToken(); void (achado ? adicionar() : testar()); } }} />
      <span class="am-ajuda">{m.maquinas_add_endereco_ajuda()}</span>
    </label>
    <label class="am-campo">
      <span class="am-rot">{m.sessao_token()}</span>
      <input class="am-input" bind:this={tokenEl} bind:value={token}
             aria-label={m.sessao_token()}
             aria-invalid={!!erro} aria-describedby={erro ? 'am-erro' : undefined}
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             disabled={ocupado}
             oninput={() => { erro = ''; achado = null; }}
             onkeydown={(e) => { if (e.key === 'Enter') void (achado ? adicionar() : testar()); }} />
      <span class="am-ajuda">{m.maquinas_add_token_ajuda({ variavel: 'CP_AUTH_TOKEN' })}</span>
    </label>
    {#if achado}
      <div class="am-achou" role="status">
        <span class="am-achou-frase">✓ {achado.identificador ? m.servidores_add_respondeu({ id: achado.identificador }) : m.servidores_add_respondeu_sem_id()}</span>
        {#if achado.idFalha}<span class="am-erro">{m.servidores_add_id_falhou({ erro: achado.idFalha })}</span>{/if}
        {#if repetida}<span class="am-ajuda">{m.servidores_add_repetida({ nome: repetida.server.label, endereco: repetida.server.baseUrl })}</span>{/if}
      </div>
      <label class="am-campo">
        <span class="am-rot">{m.servidores_add_nome()}</span>
        <input class="am-input" value={nome} aria-label={m.servidores_add_nome()} disabled={ocupado}
               oninput={(e) => (nome = e.currentTarget.value)} />
        <span class="am-ajuda">{m.servidores_add_nome_ajuda()}</span>
      </label>
    {/if}
    {#if podeFalar && !repetida}
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
          <span>{m.servidores_rec_titulo({ este: esteNome, nome: achado?.identificador || m.servidores_esta_maquina() })}</span>
          <span class="am-ajuda">{m.servidores_add_falar_grava({ este: esteNome, nome: achado?.identificador || m.servidores_esta_maquina() })}</span>
        </span>
      </label>
    {/if}
    {#if ocupado}<p class="am-status" aria-live="polite">{m.maquinas_add_testando()}</p>{/if}
    {#if erro}<p class="am-erro" id="am-erro" role="alert">{erro}</p>{/if}
  </ConfirmDialog>
{/if}

<style>
  .am-faixa { margin: 0 0 var(--space-4); padding: var(--space-3); font-size: 0.85rem; line-height: 1.45; color: var(--text-secondary);
              background: var(--surface-inset); border-left: 3px solid var(--accent); border-radius: var(--radius-sm); }
  .am-achou { display: flex; flex-direction: column; gap: var(--space-1); margin-bottom: var(--space-3); padding: var(--space-2) var(--space-3);
              border-left: 3px solid var(--success); background: var(--surface-inset); border-radius: var(--radius-sm); }
  .am-achou-frase { font-size: 0.9rem; font-weight: 600; color: var(--success); }
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
