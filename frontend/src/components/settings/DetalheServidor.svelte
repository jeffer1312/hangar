<script lang="ts">
  // Janela de UM servidor da lista: estado completo, os dois interruptores, nome/token, correção
  // de endereço e remoção. Só desenha e devolve ações por callback — quem grava é MaquinasSettings.
  import * as m from '../../paraglide/messages';
  import ModalDialog from '../ModalDialog.svelte';
  import EscopoChip from './EscopoChip.svelte';
  import { estadoDaLinha, type EstadoPeer, type LinhaMaquina } from '../../lib/maquinas';
  import type { LadoState } from '../../lib/registrarPeerDoisLados';

  interface Props {
    linha: LinhaMaquina;
    estado: EstadoPeer | undefined;
    meuIdentificador: string;
    corrige: { id: string; url: string } | null;
    idSalvando: boolean;
    idErro: string;
    onAcompanhar: (linha: LinhaMaquina, ligar: boolean) => void;
    onFalar: (linha: LinhaMaquina, ligar: boolean) => void;
    onToggleScan: (linha: LinhaMaquina, ligar: boolean) => void;
    onEditar: (linha: LinhaMaquina) => void;
    onCorrige: (url: string | null) => void;
    onTestarDeNovo: (linha: LinhaMaquina) => void;
    onRemover: (linha: LinhaMaquina) => void;
    onSalvarIdentificador: (linha: LinhaMaquina, valor: string) => void;
    onFechar: () => void;
  }
  let { linha, estado, meuIdentificador, corrige, idSalvando, idErro, onAcompanhar, onFalar, onToggleScan, onEditar, onCorrige, onTestarDeNovo, onRemover, onSalvarIdentificador, onFechar }: Props = $props();

  const e = $derived(estadoDaLinha(linha, estado));
  const url = $derived(linha.navegador?.baseUrl ?? linha.peer?.base_url ?? '');

  // Rascunho do identificador DESTA máquina (o CP_SERVER_ID dela), editável daqui porque é ele
  // que libera os recados — e sem este campo a única saída era trocar o servidor da tela inteira
  // para o aviso virar um campo. `null` = ainda não editado: mostra o que o servidor respondeu,
  // e acompanha a resposta da gravação sem precisar de efeito.
  let editado = $state<string | null>(null);
  const rascunho = $derived(editado ?? linha.identificador ?? '');
  function salvarId() {
    if (idSalvando || editado === null) return;
    const valor = rascunho.trim();
    if (valor === (linha.identificador ?? '')) return;
    onSalvarIdentificador(linha, valor);
  }

  function selo(l: LadoState | undefined): string {
    if (!l || l.estado === 'nao_configurado') return '·';
    return l.estado === 'ok' ? '✓' : '✗';
  }
  const falhou = (l: LadoState | undefined) => !!l && l.estado !== 'ok' && l.estado !== 'nao_configurado';
</script>

<ModalDialog open={true} ariaLabel={linha.nome} onClose={onFechar} className="sd-dialogo">
  <div class="mq-linha sd-rolagem" data-chave={linha.chave}>
    <div class="sd-cab">
      <span class="mq-farol" class:ok={e.farol === 'ok'} class:nao={e.farol === 'nao'} class:neutro={e.farol === 'neutro'} aria-hidden="true">
        {e.farol === 'test' ? '◌' : e.farol === 'neutro' ? '·' : '●'}
      </span>
      <div class="sd-titulo">
        <h2 class="mq-nome">{linha.nome}</h2>
        <span class="mq-url">{url}</span>
      </div>
      <button type="button" class="sd-fechar" onclick={onFechar} aria-label={m.sessao_fechar()}>✕</button>
    </div>

    {#if e.tipo !== 'ok' && e.tipo !== 'neutro' || (!linha.navegador && linha.peer)}
      <div class="sd-aviso" class:erro={e.farol === 'nao'}>
        {#if e.tipo === 'desligada'}
          <span class="mq-hint">{m.maquinas_peer_desligado()}</span>
        {:else if e.tipo === 'sem_identificador'}
          <span class="mq-hint">{m.maquinas_sem_identificador()}</span>
        {:else if e.tipo === 'nao_responde'}
          <span class="mq-hint">{m.maquinas_nao_responde()}</span>
        {:else if e.tipo === 'token_aparelho_recusado'}
          <span class="mq-hint">{m.maquinas_token_aparelho_recusado()}</span>
          {#if linha.navegador}<button type="button" class="sd-acao" onclick={() => onEditar(linha)}>{m.servidores_trocar_token()}</button>{/if}
        {:else if e.tipo === 'volta_outra_maquina'}
          <span class="mq-hint">{m.maquinas_volta_outra_maquina({ nome: linha.nome, endereco: e.volta?.url ?? '', outro: e.volta?.identificador ?? '' })}</span>
        {:else if e.tipo === 'ida_outra_maquina'}
          <span class="mq-hint">{m.maquinas_ida_outra_maquina({ nome: linha.nome, endereco: linha.peer?.base_url ?? '', outro: e.ida?.identificador ?? '' })}</span>
        {:else if e.tipo === 'testando'}
          <span class="mq-hint">{m.peers_estado_testando()}</span>
        {:else if e.tipo === 'token_recusado'}
          <span class="mq-hint">{m.maquinas_volta_token_recusado()}</span>
          {#if linha.navegador}<button type="button" class="sd-acao" onclick={() => onEditar(linha)}>{m.servidores_trocar_token()}</button>{/if}
        {:else if e.tipo === 'parcial'}
          <span class="mq-hint">
            {m.peers_estado_parcial()}
            <span class="pr-lados">
              <span class="pr-lado" class:ok={e.ida?.estado === 'ok'} class:nao={falhou(e.ida)} title={falhou(e.ida) ? e.ida?.motivo : undefined}>{selo(e.ida)} {m.peers_lado_ida()}</span>
              <span class="pr-lado" class:ok={e.volta?.estado === 'ok'} class:nao={falhou(e.volta)} title={falhou(e.volta) ? e.volta?.motivo : undefined}>{selo(e.volta)} {m.peers_lado_volta()}</span>
            </span>
          </span>
        {:else if e.tipo === 'volta_sem_medir'}
          <span class="mq-hint">{m.maquinas_volta_sem_medir()}</span>
        {:else if e.tipo === 'volta_sem_registro'}
          <span class="mq-hint">{m.maquinas_volta_sem_registro()}</span>
        {/if}
        <!-- Independente do estado acima: sem entrada neste aparelho, a instrução acionável é
             sempre informar o token, mesmo com um teste já rodado. -->
        {#if !linha.navegador && linha.peer}
          <span class="mq-hint">{m.maquinas_so_no_servidor()}</span>
          <button type="button" class="sd-acao" onclick={() => onAcompanhar(linha, true)}>{m.servidores_informar_token()}</button>
        {/if}
      </div>
    {/if}

    {#if corrige?.id === linha.identificador}
      <div class="corrige">
        <!-- O endereço que falhou é o da VOLTA: o que o peer guardou DESTA máquina, e não o
             endereço do peer. Era o do peer aqui, e a frase acusava o endereço certo. -->
        <p>{m.peers_corrige_1({ nome: linha.nome, endereco: e.volta?.url ?? corrige.url })}</p>
        <p><b>{m.peers_corrige_pergunta({ nome: linha.nome })}</b></p>
        <input class="corrige-input" value={corrige.url}
               aria-label={m.peers_corrige_pergunta({ nome: linha.nome })}
               oninput={(ev) => onCorrige(ev.currentTarget.value)} />
        <div class="acoes">
          <button type="button" class="btn primaria" onclick={() => onTestarDeNovo(linha)}>{m.peers_testar_novamente()}</button>
          <button type="button" class="btn" onclick={() => onCorrige(null)}>{m.peers_so_ida()}</button>
        </div>
      </div>
    {/if}

    <p class="sd-grupo">{m.servidores_grupo_aparelho()}</p>
    <div class="sd-cartao">
      <label class="sd-campo">
        <span class="sd-rot">{m.maquinas_acompanhar()}</span>
        <!-- A caixa não muda sozinha: volta ao dado e avisa o dono, que decide (confirmação,
             pedir token). Sem isto ela ficava marcada com o gesto recusado. -->
        <input type="checkbox" class="switch mq-acompanhar" checked={!!linha.navegador}
               onchange={(ev) => { const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onAcompanhar(linha, ligar); }} />
      </label>
      {#if linha.navegador}
        <div class="sd-campo">
          <span class="sd-rot">{m.servidores_nome_token()}</span>
          <button type="button" class="mq-editar" aria-label={m.servidor_editar_aria({ nome: linha.nome })} onclick={() => onEditar(linha)}>{m.config_motores_editar()}</button>
        </div>
      {/if}
    </div>

    <p class="sd-grupo">{m.servidores_grupo_entre()}</p>
    <div class="sd-cartao">
      <!-- O identificador é do .env DELE, não deste aparelho — por isso o chip `env` e não o de
           servidor, e por isso só aparece quando há token dele aqui: sem credencial não há como
           gravar lá. Vem antes do interruptor porque é a condição dele. -->
      {#if linha.navegador}
        <label class="sd-campo">
          <span class="sd-rot">{m.peers_identificador()} <EscopoChip escopo="env" />
            <small>{linha.identificador
              ? m.maquinas_identificador_remoto_legenda({ nome: linha.nome })
              : m.peers_identificador_dica({ exemplos: 'casa, notebook' })}</small>
          </span>
          <input class="sd-id" class:vazio={!linha.identificador} value={rascunho}
                 placeholder={m.peers_identificador_placeholder()}
                 autocomplete="off" autocorrect="off" autocapitalize="off" spellcheck={false}
                 readonly={idSalvando}
                 oninput={(ev) => (editado = ev.currentTarget.value)}
                 onkeydown={(ev) => { if (ev.key === 'Enter') salvarId(); }}
                 onblur={salvarId} />
        </label>
        {#if idErro}<p class="sd-motivo erro" role="alert">{idErro}</p>{/if}
      {/if}
      <label class="sd-campo">
        <span class="sd-rot">{m.maquinas_falar()} <EscopoChip escopo="servidor" /></span>
        <input type="checkbox" class="switch mq-falar" checked={!!linha.peer}
               disabled={!meuIdentificador || !linha.identificador}
               onchange={(ev) => { const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onFalar(linha, ligar); }} />
      </label>
      {#if !meuIdentificador}
        <p class="sd-motivo">{m.peers_aviso_nao_definido()}</p>
      {/if}
      {#if linha.peer}
        <label class="sd-campo">
          <span class="sd-rot">{m.maquinas_varredura()} <EscopoChip escopo="servidor" />
            <small>{m.maquinas_varredura_legenda()}</small>
          </span>
          <input type="checkbox" class="switch mq-varredura" checked={linha.peer.enabled !== false}
                 onchange={(ev) => { const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onToggleScan(linha, ligar); }} />
        </label>
      {/if}
    </div>

    <!-- O ✕ alcança os lados que ESTA linha tem, e são os dois campos que decidem: `peer` diz se o
         registro do servidor sai, `navegador` diz se há entrada aqui E se o lado de lá é
         alcançável. O aria-label segue as mesmas condições porque substitui o nome acessível. -->
    <div class="sd-rodape">
      <button type="button" class="mq-remover" aria-label={!linha.peer ? m.maquinas_remover_aria_local({ nome: linha.nome }) : linha.navegador ? m.maquinas_remover_aria({ nome: linha.nome }) : m.maquinas_remover_aria_servidor({ nome: linha.nome })} onclick={() => onRemover(linha)}>{m.lista_remover()}{#if linha.peer}<EscopoChip escopo="servidor" />{/if}</button>
    </div>
  </div>
</ModalDialog>

<style>
  /* Quem rola é o miolo, não o diálogo: o vidro do ModalDialog é um ::before do tamanho da caixa, e
     com o próprio diálogo rolando a parte de baixo do conteúdo ficava sem fundo. */
  :global(.modal-backdrop .modal-dialog.sd-dialogo) { width: min(520px, 94vw); max-height: 90dvh; overflow: hidden; display: flex; flex-direction: column; container-type: inline-size; }
  /* Tela grande: o diálogo usa a largura que sobra. Aqui a medida é da JANELA mesmo, não de um
     painel — é um sobreposto em cima de tudo. Abaixo de 900px nada muda, então celular e PWA
     seguem com os mesmos 520px de sempre. */
  @media (min-width: 900px) {
    :global(.modal-backdrop .modal-dialog.sd-dialogo) { width: min(760px, 88vw); }
  }
  :global(.sd-rolagem) { position: relative; min-height: 0; overflow: auto; overscroll-behavior: contain; padding: var(--space-5); }
  .mq-linha { display: flex; flex-direction: column; }
  .sd-cab { display: flex; align-items: flex-start; gap: var(--space-3); margin-bottom: var(--space-3); }
  .mq-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; line-height: 1.9; color: var(--text-muted); }
  .mq-farol.ok { color: var(--success); }
  .mq-farol.nao { color: var(--error); }
  .sd-titulo { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .mq-nome { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); }
  .mq-url { font-family: var(--font-mono); font-size: 12px; color: var(--text-muted); word-break: break-all; }
  .sd-fechar { width: 36px; height: 36px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm); color: var(--text-muted); }
  .sd-fechar:hover { background: var(--bg-hover); color: var(--text-primary); }

  .sd-aviso { display: flex; flex-direction: column; align-items: flex-start; gap: var(--space-2); padding: var(--space-3); margin-bottom: var(--space-2);
              border-left: 3px solid var(--warning); border-radius: var(--radius-md); background: var(--surface-inset); }
  .sd-aviso.erro { border-left-color: var(--error); }
  .mq-hint { font-size: var(--text-sm); line-height: 1.4; color: var(--text-secondary); }
  .sd-acao { min-height: 0; height: 32px; padding: 0 var(--space-3); border-radius: var(--radius-sm); border: 1px solid var(--border-default); color: var(--accent); font-size: var(--text-sm); }
  .pr-lados { display: flex; gap: var(--space-2); margin-top: var(--space-1); }
  .pr-lado { display: inline-flex; align-items: center; gap: 4px; font-size: 11px; color: var(--text-muted);
             padding: 2px var(--space-2); border-radius: var(--radius-full); border: 1px solid var(--border-subtle); }
  .pr-lado.ok { color: var(--success); }
  .pr-lado.nao { color: var(--error); }

  .sd-grupo { margin: var(--space-4) var(--space-1) var(--space-2); font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .sd-cartao { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; }
  .sd-campo { display: flex; align-items: center; gap: var(--space-3); min-height: 52px; padding: var(--space-2) var(--space-3); }
  .sd-campo + .sd-campo { border-top: 1px solid var(--border-subtle); }
  .sd-rot { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); }
  .sd-motivo { margin: 0; padding: 0 var(--space-3) var(--space-3); font-size: var(--text-xs); color: var(--warning); }
  .sd-motivo.erro { color: var(--error); }
  .sd-rot small { display: block; margin-top: 2px; font-size: var(--text-xs); color: var(--text-muted); line-height: 1.35; }
  .sd-id { width: 9rem; flex-shrink: 0; height: 36px; padding: 0 var(--space-3); box-sizing: border-box;
           background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
           color: var(--text-primary); font-family: var(--font-mono); font-size: var(--text-sm); }
  .sd-id.vazio { border-color: var(--warning); }
  .mq-editar { min-height: 0; height: 32px; padding: 0 var(--space-3); border-radius: var(--radius-sm); border: 1px solid var(--border-default); color: var(--text-primary); font-size: var(--text-sm); }
  .mq-editar:hover { background: var(--bg-hover); }

  .sd-rodape { display: flex; justify-content: flex-end; margin-top: var(--space-4); }
  .mq-remover { display: inline-flex; align-items: center; gap: var(--space-2); min-height: 0; height: 36px; padding: 0 var(--space-3); border-radius: var(--radius-sm); color: var(--error); font-size: var(--text-sm); }
  .mq-remover:hover { background: rgba(255, 69, 58, 0.1); }

  .corrige { margin-bottom: var(--space-2); padding: var(--space-3); border: 1px solid var(--border-default); border-left: 3px solid var(--warning); border-radius: var(--radius-md); }
  .corrige p { margin: 0 0 var(--space-2); font-size: var(--text-xs); color: var(--text-secondary); line-height: 1.45; }
  .corrige b { color: var(--text-primary); font-weight: 600; }
  .corrige-input { width: 100%; height: 36px; padding: 0 var(--space-3); box-sizing: border-box;
                   background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
                   color: var(--text-primary); font-family: var(--font-mono); font-size: var(--text-sm); }
  .acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); margin-top: var(--space-3); }
  .btn { height: 36px; min-height: 0; padding: 0 var(--space-4); border-radius: var(--radius-sm); border: 1px solid var(--border-subtle);
         background: var(--surface-raised); color: var(--text-primary); font-size: var(--text-sm); font-family: inherit; }
  .btn.primaria { background: var(--accent); border-color: var(--accent); color: #fff; }

  @container (max-width: 420px) {
    .sd-acao, .mq-editar, .mq-remover, .btn { height: 44px; }
  }
</style>
