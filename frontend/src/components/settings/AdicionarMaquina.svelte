<script lang="ts">
  // Ligar esta máquina em outra: endereço e token em campos separados, o endereço normalizado
  // (IP, nome ou link inteiro), e um GET no candidato ANTES de gravar — nada toca o storage até o
  // servidor responder. O reload no sucesso é deliberado: a lista nova muda ativo/token e o SSE do
  // store precisa renascer limpo.
  import * as m from '../../paraglide/messages';
  import ConfirmDialog from '../ConfirmDialog.svelte';
  import QrScanner from '../QrScanner.svelte';
  import { normalizarEndereco } from '../../lib/url';
  import { getConfigForServer } from '../../lib/api';
  import { addServer } from '../../lib/auth';
  import { getIdentificador } from '../../lib/peers';
  import { registrarPeerDoisLados } from '../../lib/registrarPeerDoisLados';
  import type { Server } from '../../lib/auth';

  interface Props {
    fallbackFocus?: HTMLElement | null;
    onFechar: () => void;
    apiTarget?: Server | null;
    podeFalar?: boolean;
    enderecoInicial?: string;
  }
  let { fallbackFocus = null, onFechar, apiTarget = null, podeFalar = false, enderecoInicial = '' }: Props = $props();

  let endereco = $state(enderecoInicial);
  let token = $state('');
  let erro = $state('');
  let ocupado = $state(false);
  let scanning = $state(false);
  let enderecoEl = $state<HTMLInputElement | null>(null);
  let falar = $state(false);   // é uma pergunta: quem quer, marca

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
    ocupado = true;
    erro = '';
    let base = n.base;
    try {
      await getConfigForServer({ id: 'candidato', label: base, baseUrl: base, token: tok });
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
        await getConfigForServer({ id: 'candidato', label: base, baseUrl: base, token: tok });
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
        const { identificador } = await getIdentificador({ id: 'candidato', label: base, baseUrl: base, token: tok });
        if (identificador) await registrarPeerDoisLados(apiTarget, { id: identificador, base_url: base, token: tok });
      } catch {
        // a lista dirá "só uma das pontas responde"; o que a pessoa pediu — acompanhar — segue.
      }
    }
    addServer(base, tok);
    window.location.reload();
    ocupado = false;
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
  <ConfirmDialog title={m.sessao_adicionar_servidor()} aria={m.sessao_adicionar_servidor()} role="dialog" wide
    {fallbackFocus} initialFocus={enderecoEl}
    onClose={fechar}
    actions={[
      { label: m.sessao_escanear_qr(), disabled: ocupado, onClick: () => (scanning = true) },
      { label: m.maquinas_add_testar(), kind: 'primary', disabled: !podeTestar, onClick: testarEAdicionar },
    ]}>
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
      <input class="am-input" bind:value={token}
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
        <input class="switch am-falar" type="checkbox" bind:checked={falar} disabled={ocupado} />
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
