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

  interface Props {
    fallbackFocus?: HTMLElement | null;
    onFechar: () => void;
  }
  let { fallbackFocus = null, onFechar }: Props = $props();

  let endereco = $state('');
  let token = $state('');
  let erro = $state('');
  let ocupado = $state(false);
  let scanning = $state(false);
  let enderecoEl = $state<HTMLInputElement | null>(null);

  // Link de pareamento colado inteiro: o token vai para o campo dele e o endereço fica só a origem.
  // Roda no blur, não no input — normalizar a cada tecla reescreveria o que a pessoa está digitando.
  function separarToken() {
    const n = normalizarEndereco(endereco);
    if (n?.token) { token = n.token; endereco = n.base; }
  }

  // Responde? Grava. Nome com ponto veio como https por dedução; se ninguém atendeu lá, o mesmo
  // nome em http na porta padrão é a segunda e última tentativa (FQDN de rede local).
  async function testarEAdicionar() {
    if (ocupado) return;
    const n = normalizarEndereco(endereco);
    if (!n) { erro = m.maquinas_add_erro_endereco(); return; }
    const tok = (n.token ?? token).trim();
    if (!tok || /\s/.test(tok)) { erro = m.maquinas_add_erro_token(); return; }
    ocupado = true;
    erro = '';
    try {
      let base = n.base;
      try {
        await getConfigForServer({ id: 'candidato', label: base, baseUrl: base, token: tok });
      } catch (e) {
        if (!n.alternativa) throw e;
        base = n.alternativa;
        await getConfigForServer({ id: 'candidato', label: base, baseUrl: base, token: tok });
      }
      addServer(base, tok);
      window.location.reload();
    } catch (e) {
      erro = e instanceof Error ? `${m.falha_conexao()}: ${e.message}` : m.erro_desconhecido();
    } finally {
      ocupado = false;
    }
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
             autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
             disabled={ocupado}
             oninput={() => (erro = '')}
             onkeydown={(e) => { if (e.key === 'Enter') void testarEAdicionar(); }} />
      <span class="am-ajuda">{m.maquinas_add_token_ajuda({ variavel: 'CP_AUTH_TOKEN' })}</span>
    </label>
    {#if ocupado}<p class="am-status" aria-live="polite">{m.maquinas_add_testando()}</p>{/if}
    {#if erro}<p class="am-erro" role="alert">{erro}</p>{/if}
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
  .am-status { font-size: 0.85rem; color: var(--text-muted); margin: 0; }
  .am-erro { color: var(--error); font-size: 0.85rem; margin: 0; }
</style>
