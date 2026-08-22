<script lang="ts">
  import BottomSheet from './BottomSheet.svelte';
  import { validarPareamento } from '../lib/auth';
  import { vaultPush } from '../lib/vaultPush.svelte';
  import type { Server } from '../lib/auth';
  import * as m from '../paraglide/messages';

  // Edicao de servidor numa folha propria, no lugar dos dois editores INLINE que viviam na linha da
  // lista (renomear e trocar token). A linha tem ~40px de altura e divide o espaco com ponto, rotulo
  // e tres botoes de 30px: no celular nao dava pra ver o que estava gravado, so um campo abria por
  // vez, e o campo de token nascia VAZIO — quem queria so trocar o token nao tinha com o que
  // comparar. Aqui os tres valores aparecem juntos, com o token mascarado e um botao pra revelar.
  interface Props {
    open: boolean;
    server: Server | null;
    onClose: () => void;
    onRename: (id: string, label: string) => void;
    onUpdateToken: (id: string, token: string) => boolean;
  }
  let { open, server, onClose, onRename, onUpdateToken }: Props = $props();

  let label = $state('');
  let token = $state('');
  let revelado = $state(false);
  let erro = $state('');
  let aviso = $state('');
  let tokenEl = $state<HTMLInputElement | null>(null);

  // Repoe os campos a cada abertura E a cada troca de servidor: a folha fica montada, entao sem isto
  // o texto digitado numa edicao abandonada reaparecia na proxima — no campo de TOKEN, com cara de
  // valor gravado.
  let ultimo = '';
  $effect(() => {
    const chave = open && server ? `${server.id}:${server.token}` : '';
    if (chave === ultimo) return;
    ultimo = chave;
    label = server?.label ?? '';
    token = server?.token ?? '';
    revelado = false;
    erro = '';
    aviso = '';
  });

  function salvar() {
    if (!server) return;
    const nome = label.trim();
    const texto = token.trim();
    // Vazio nao e "nao mexe": o campo ja vem preenchido, entao em branco significa que o usuario
    // apagou. Gravar isso desautenticaria o servidor calado (updateServer trata vazio como "manter",
    // e a tela mentiria dizendo que salvou o que esta na tela).
    if (!texto) {
      erro = m.servidor_token_vazio();
      tokenEl?.focus();
      return;
    }

    let tokenFinal = texto;
    let outroHost = false;
    if (texto !== server.token) {
      // Aceita a URL de pareamento inteira (quem cola do QR nao extrai nada na mao) e RECUSA lixo:
      // URL torta, sem ?token=, token com espaco. Gravar aqui era o 401 sem pista que chegava depois.
      const parsed = validarPareamento(texto, { aceitarTokenCru: true });
      if (!parsed) {
        erro = texto.includes('://') ? m.servidor_url_invalida() : m.servidor_token_invalido();
        tokenEl?.focus();
        return;
      }
      // So o TOKEN: colar a URL de outra maquina nao pode reapontar calado um servidor ja cadastrado.
      tokenFinal = parsed.token;
      outroHost = !!parsed.base
        && parsed.base.replace(/\/+$/, '') !== server.baseUrl.replace(/\/+$/, '');
    }

    if (nome !== server.label) onRename(server.id, nome);
    if (tokenFinal !== server.token) {
      vaultPush.clear();                        // tentativa NOVA: zera o resultado do push antigo
      if (!onUpdateToken(server.id, tokenFinal)) {
        // false = o id sumiu (removido noutra aba/aparelho entre abrir e salvar). Raro, mas
        // indistinguivel de sucesso se ficasse calado.
        erro = m.servidor_nao_existe();
        return;
      }
    }
    if (outroHost) {
      // Salvou, mas o endereco NAO mudou: fica aberta pra o usuario ler o que aconteceu com a URL
      // que ele colou. Fechar aqui esconderia justamente a parte que ele nao esperava.
      erro = '';
      aviso = m.servidor_token_trocado({ url: server.baseUrl });
      token = tokenFinal;
      return;
    }
    onClose();
  }
</script>

<BottomSheet {open} {onClose} ariaLabel={m.servidor_editar_titulo()} wide centered>
  <h2 class="sheet-title">{m.servidor_editar_titulo()}</h2>

  {#if server}
    <label class="se-campo">
      <span class="se-rotulo">{m.comum_nome()}</span>
      <input class="se-input" bind:value={label} autocomplete="off"
             onkeydown={(e) => { if (e.key === 'Enter') salvar(); }} />
    </label>

    <div class="se-campo">
      <span class="se-rotulo">{m.servidor_campo_endereco()}</span>
      <!-- Somente leitura de proposito: trocar o host de um servidor ja cadastrado e outra coisa
           (credencial, historico e nome ficam apontando pra maquina errada). Aqui ele existe pra
           ser LIDO — era o que faltava pra saber qual "Casa" da lista e qual. -->
      <p class="se-fixo">{server.baseUrl}</p>
      <p class="se-ajuda">{m.servidor_endereco_fixo()}</p>
    </div>

    <div class="se-campo">
      <span class="se-rotulo">{m.servidor_campo_token()}</span>
      <div class="se-token">
        <input
          class="se-input"
          type={revelado ? 'text' : 'password'}
          bind:value={token}
          bind:this={tokenEl}
          autocomplete="off"
          autocapitalize="off"
          autocorrect="off"
          spellcheck="false"
          aria-label={m.servidor_novo_token_aria({ nome: server.label })}
          aria-invalid={erro ? true : undefined}
          aria-describedby={erro ? 'se-err' : undefined}
          onkeydown={(e) => { if (e.key === 'Enter') salvar(); }}
        />
        <button class="se-olho" type="button" onclick={() => (revelado = !revelado)}
                aria-pressed={revelado}
                aria-label={revelado ? m.servidor_ocultar_token() : m.servidor_mostrar_token()}>
          {revelado ? '🙈' : '👁'}
        </button>
      </div>
      <p class="se-ajuda">{m.servidor_token_ajuda()}</p>
    </div>

    {#if erro}<p id="se-err" class="se-erro" role="alert">{erro}</p>{/if}
    {#if aviso}<p class="se-aviso" role="status">{aviso}</p>{/if}

    <div class="se-acoes">
      <button class="se-btn" type="button" onclick={onClose}>{m.comum_cancelar()}</button>
      <button class="se-btn se-salvar" type="button" onclick={salvar}>{m.ctx_salvar()}</button>
    </div>
  {/if}
</BottomSheet>

<style>
  .sheet-title { font-size: var(--text-xl); font-weight: 600; color: var(--text-primary); margin-bottom: var(--space-4); }
  .se-campo { display: block; margin-bottom: var(--space-4); }
  .se-rotulo {
    display: block; margin-bottom: var(--space-1);
    font-size: var(--text-xs); font-weight: 600; letter-spacing: 0.04em; text-transform: uppercase;
    color: var(--text-muted);
  }
  /* 16px no campo: abaixo disso o iOS dá zoom ao focar e a folha sai do lugar. */
  .se-input {
    width: 100%; min-height: 44px; padding: 0 var(--space-3);
    background: var(--surface-inset); border: 1px solid var(--border-subtle); border-radius: var(--radius-md, 8px);
    color: var(--text-primary); font-family: var(--font-ui); font-size: 16px; outline: none;
  }
  .se-input:focus-visible { border-color: var(--accent); outline: 2px solid var(--accent); outline-offset: -2px; }
  .se-token { display: flex; align-items: center; gap: var(--space-2); }
  .se-olho {
    flex-shrink: 0; width: 44px; height: 44px; min-height: 44px;
    border: 1px solid var(--border-subtle); border-radius: var(--radius-md, 8px);
    background: var(--surface-raised); color: var(--text-secondary); font-size: var(--text-base);
  }
  .se-fixo {
    margin: 0; padding: var(--space-2) var(--space-3);
    background: var(--surface-inset); border-radius: var(--radius-md, 8px);
    color: var(--text-secondary); font-family: var(--font-mono); font-size: var(--text-sm);
    overflow-wrap: anywhere;
  }
  .se-ajuda { margin: var(--space-1) 0 0; font-size: var(--text-xs); color: var(--text-muted); line-height: 1.4; }
  .se-erro { margin: 0 0 var(--space-3); font-size: var(--text-sm); color: var(--error); line-height: 1.4; }
  .se-aviso { margin: 0 0 var(--space-3); font-size: var(--text-sm); color: var(--warning); line-height: 1.4; }
  .se-acoes { display: flex; justify-content: flex-end; gap: var(--space-2); margin-top: var(--space-4); }
  .se-btn {
    min-height: 44px; padding: 0 var(--space-4); border-radius: var(--radius-md, 8px);
    border: 1px solid var(--border-subtle); background: var(--surface-raised);
    color: var(--text-primary); font-size: var(--text-sm); font-weight: 600;
  }
  .se-salvar { border-color: var(--accent); background: var(--accent-dim); color: var(--accent); }
  button:focus-visible { outline: 2px solid var(--accent); outline-offset: 2px; }
</style>
