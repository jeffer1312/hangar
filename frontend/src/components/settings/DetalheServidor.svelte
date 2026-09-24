<script lang="ts">
  // Janela de UMA máquina: o que este aparelho guarda dela, os recados entre ela e o servidor
  // escolhido (um resultado só, com o botão que resolve) e o avançado. Só desenha e devolve ações
  // por callback — quem grava é MaquinasSettings.
  import { onMount } from 'svelte';
  import * as m from '../../paraglide/messages';
  import ModalDialog from '../ModalDialog.svelte';
  import EscopoChip from './EscopoChip.svelte';
  import { estadoDaLinha, recadosCard, sessionsState, type EstadoPeer, type LinhaMaquina } from '../../lib/maquinas';
  import type { EnderecoAlcance } from '../../lib/alcance';
  import type { Server } from '../../lib/auth';

  interface Props {
    linha: LinhaMaquina;
    estado: EstadoPeer | undefined;
    meuIdentificador: string;
    esteNome: string;
    enderecos: EnderecoAlcance[];
    idSalvando: boolean;
    idErro: string;
    tokenSalvando: boolean;
    tokenErro: string;
    erro?: string;
    onAcompanhar: (linha: LinhaMaquina, ligar: boolean) => void;
    onFalar: (linha: LinhaMaquina, ligar: boolean) => void;
    onToggleScan: (linha: LinhaMaquina, ligar: boolean) => void;
    onEditar: (s: Server) => void;
    onTirar: (s: Server) => void;
    onInformarToken: (linha: LinhaMaquina, token: string) => void;
    onTestar: (linha: LinhaMaquina) => void;
    onUsarEndereco: (linha: LinhaMaquina, url: string) => void;
    onAbrirEste: () => void;
    onRemover: (linha: LinhaMaquina) => void;
    onSalvarIdentificador: (linha: LinhaMaquina, valor: string) => void;
    onFechar: () => void;
  }
  let { linha, estado, meuIdentificador, esteNome, enderecos, idSalvando, idErro, tokenSalvando, tokenErro, erro = '',
        onAcompanhar, onFalar, onToggleScan, onEditar, onTirar, onInformarToken, onTestar, onUsarEndereco,
        onAbrirEste, onRemover, onSalvarIdentificador, onFechar }: Props = $props();

  // Abrir o detalhe mede de novo: o resultado de uma abertura antiga não vale como estado de agora.
  onMount(() => { if (linha.peer && linha.peer.enabled !== false) onTestar(linha); });

  const e = $derived(estadoDaLinha(linha, estado));
  const card = $derived(recadosCard(linha, estado, meuIdentificador));
  const sessoes = $derived(sessionsState(linha, estado));
  const nome = $derived(linha.nome);
  const ms = (v: number | null | undefined) => (v == null ? '' : String(Math.round(v)));

  // Rascunho do identificador DELA (o CP_SERVER_ID do .env dela). `null` = não editado.
  let editado = $state<string | null>(null);
  const rascunho = $derived(editado ?? linha.identificador ?? '');
  function salvarId() {
    if (idSalvando || editado === null) return;
    const valor = rascunho.trim();
    if (valor === (linha.identificador ?? '')) return;
    onSalvarIdentificador(linha, valor);
  }

  // Token pedido no lugar: ligar "Mostrar as sessões dele" sem entrada aqui abre este campo.
  let pedindoToken = $state(false);
  let token = $state('');
  const salvarToken = () => { if (!tokenSalvando && token.trim()) onInformarToken(linha, token.trim()); };

  // Escolha do endereço por onde ELA chega no servidor escolhido. As opções são os endereços que
  // o próprio servidor mediu; a primeira que responde vem marcada.
  const opcoes = $derived.by(() => {
    const vistos = new Set<string>();
    return enderecos
      .filter((x) => x.tipo !== 'nesta_maquina' && !vistos.has(x.url) && vistos.add(x.url))
      .sort((a, b) => Number(b.estado === 'ok') - Number(a.estado === 'ok') || (a.tempo_ms ?? 1e9) - (b.tempo_ms ?? 1e9));
  });
  let escolhida = $state<string | null>(null);
  let outro = $state('');
  const urlEscolhida = $derived(escolhida === '' ? outro.trim() : escolhida ?? opcoes[0]?.url ?? '');
  let soIda = $state(false);
  let trocando = $state(false);
  const TIPO: Record<string, () => string> = {
    rede_local: m.acesso_rede_local, tailscale: m.acesso_tailscale, publico: m.acesso_publico, nesta_maquina: m.acesso_nesta_maquina,
  };

  function fraseSessoes(): string {
    switch (sessoes) {
      case 'aparecem': return m.servidores_sessoes_aparecem();
      case 'testando': return m.acesso_testando();
      case 'nao_responde': return m.servidores_nao_respondeu();
      case 'token_recusado': return m.servidores_sessoes_token_recusado();
      case 'sem_token': return m.servidores_sem_token_aqui();
      case 'desligada': return linha.navegador ? m.servidores_sessoes_aparecem() : m.servidores_sem_token_aqui();
      case 'desligada_aqui': return m.servidores_sessoes_desligadas();
    }
  }
</script>

{#snippet escolha(botaoSoIda: boolean)}
  <div class="sd-opcoes" role="radiogroup" aria-label={m.servidores_rec_escolha_aria({ nome })}>
    {#each opcoes as o (o.url)}
      <label class="sd-opcao" class:on={urlEscolhida === o.url && escolhida !== ''}>
        <input type="radio" name="sd-end" checked={urlEscolhida === o.url && escolhida !== ''} onchange={() => (escolhida = o.url)} />
        <span>{TIPO[o.tipo]()}<small class="mq-url">{o.url} · {o.estado === 'ok' ? m.servidores_rec_responde({ ms: ms(o.tempo_ms) }) : m.servidores_rec_nao_respondeu()}</small></span>
      </label>
    {/each}
    <label class="sd-opcao" class:on={escolhida === ''}>
      <input type="radio" name="sd-end" checked={escolhida === ''} onchange={() => (escolhida = '')} />
      <span class="sd-outro">{m.servidores_rec_outro()}
        <input class="sd-in" value={outro} placeholder="https://…" autocomplete="off" autocapitalize="off" spellcheck={false}
               aria-label={m.servidores_rec_outro()}
               onfocus={() => (escolhida = '')} oninput={(ev) => (outro = ev.currentTarget.value)} /></span>
    </label>
  </div>
  <div class="sd-acoes">
    <button type="button" class="sd-btn pri" disabled={!urlEscolhida}
            onclick={() => { trocando = false; soIda = false; onUsarEndereco(linha, urlEscolhida); }}>{m.servidores_rec_usar()}</button>
    {#if botaoSoIda}<button type="button" class="sd-btn" onclick={() => (soIda = true)}>{m.peers_so_ida()}</button>{/if}
    {#if trocando}<button type="button" class="sd-btn" onclick={() => (trocando = false)}>{m.comum_cancelar()}</button>{/if}
  </div>
{/snippet}

<ModalDialog open={true} ariaLabel={nome} onClose={onFechar} className="sd-dialogo">
  <div class="mq-linha sd-rolagem" data-chave={linha.chave}>
    <div class="sd-cab">
      <div class="sd-titulo">
        <h2 class="mq-nome">{nome}{#if linha.identificador && linha.identificador !== nome}<span class="sd-chip">{linha.identificador}</span>{/if}</h2>
        <span class="sd-testado">{estado?.testando || card === 'testando' || sessoes === 'testando' ? m.acesso_testando()
          : estado?.em ? m.servidores_testado_as({ hora: new Date(estado.em).toLocaleTimeString([], { hour: '2-digit', minute: '2-digit' }) })
          : m.servidores_testado_agora()}</span>
      </div>
      <button type="button" class="sd-fechar" onclick={onFechar} aria-label={m.sessao_fechar()}>✕</button>
    </div>

    <p class="sd-grupo">{m.servidores_grupo_aparelho()}</p>
    <div class="sd-cartao">
      <label class="sd-campo">
        <span class="sd-rot">{m.maquinas_acompanhar()}
          <small>{tokenSalvando ? m.maquinas_add_testando() : linha.navegador ? fraseSessoes() : m.servidores_sem_token_aqui()}</small>
        </span>
        <!-- Desligar só esconde: a entrada e o token ficam. Ligar sem entrada aqui usa o token que o
             servidor guarda; o campo só aparece se isso falhar. -->
        <input type="checkbox" class="switch mq-acompanhar" checked={linha.navegadores.some((s) => !s.disabled) || tokenSalvando}
               disabled={tokenSalvando}
               onchange={(ev) => {
                 const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar;
                 if (ligar && !linha.navegador) { pedindoToken = true; onInformarToken(linha, ''); }
                 else if (!ligar && !linha.navegador) pedindoToken = false;
                 else onAcompanhar(linha, ligar);
               }} />
      </label>
      {#if pedindoToken && !linha.navegador && !tokenSalvando && tokenErro}
        <div class="sd-res">
          <p>{m.maquinas_add_token_ajuda({ variavel: 'CP_AUTH_TOKEN' })}</p>
          <input class="sd-in" value={token} placeholder={m.sessao_token()} aria-label={m.sessao_token()}
                 autocomplete="off" autocapitalize="off" spellcheck={false} readonly={tokenSalvando}
                 oninput={(ev) => (token = ev.currentTarget.value)} onkeydown={(ev) => { if (ev.key === 'Enter') salvarToken(); }} />
          {#if tokenErro}<p class="sd-erro" role="alert">{tokenErro}</p>{/if}
          <div class="sd-acoes"><button type="button" class="sd-btn pri" disabled={tokenSalvando || !token.trim()} onclick={salvarToken}>{tokenSalvando ? m.maquinas_add_testando() : m.ctx_salvar()}</button></div>
        </div>
      {/if}
      {#each linha.navegadores as s (s.id)}
        <div class="sd-campo">
          <span class="sd-rot">{s.label}
            <small class="mq-url">{s.baseUrl}{#if linha.motivos[s.id] === 'sem_resposta'} · {m.servidores_nao_respondeu()}{:else if linha.motivos[s.id] === 'token'} · {m.servidores_curto_token_recusado()}{/if}</small>
          </span>
          <button type="button" class="mq-editar" aria-label={m.servidor_editar_aria({ nome: s.label })} onclick={() => onEditar(s)}>{m.servidores_nome_token()}</button>
          {#if linha.navegadores.length > 1}
            <button type="button" class="sd-tirar" aria-label={m.servidores_tirar_aria({ nome: s.label })} onclick={() => onTirar(s)}>{m.servidores_tirar()}</button>
          {/if}
        </div>
      {/each}
      {#if linha.navegadores.length > 1}<p class="sd-dup">{m.servidores_guardado_explica({ n: linha.navegadores.length })}</p>{/if}
    </div>

    <p class="sd-grupo">{m.maquinas_falar()} <EscopoChip escopo="servidor" /></p>
    <div class="sd-cartao">
      <label class="sd-campo">
        <span class="sd-rot">{m.servidores_rec_titulo({ este: esteNome, nome })}
          <small>{card === 'desligado' ? m.servidores_rec_desligado({ este: esteNome, nome }) : m.servidores_rec_legenda({ este: esteNome, nome })}</small>
        </span>
        <input type="checkbox" class="switch mq-falar" checked={!!linha.peer}
               disabled={!meuIdentificador || !linha.identificador}
               onchange={(ev) => { const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onFalar(linha, ligar); }} />
      </label>

      {#if card !== 'desligado'}
        <div class="sd-res" class:ok={card === 'ok'} class:av={['endereco_volta', 'falta_token', 'sem_id_dele', 'sem_meu_id', 'sem_registro', 'pausada'].includes(card)}
             class:nao={['token_recusado', 'volta_outra', 'ida_outra', 'ida_falhou', 'sem_resposta'].includes(card)} role="status">
          {#if card === 'ok'}
            <span class="sd-frase ok">✓ {m.servidores_rec_ok()}</span>
          {:else if card === 'testando'}
            <span class="sd-frase">◌ {m.peers_estado_testando()}</span>
          {:else if card === 'sem_meu_id'}
            <span class="sd-frase">{m.servidores_rec_sem_meu_id({ este: esteNome })}</span>
            <p>{m.servidores_rec_sem_meu_id_p({ este: esteNome })}</p>
            <div class="sd-acoes"><button type="button" class="sd-btn pri" onclick={onAbrirEste}>{m.servidores_rec_dar_nome()}</button></div>
          {:else if card === 'sem_id_dele'}
            <span class="sd-frase">{m.servidores_rec_sem_id_dele({ nome })}</span>
            <p>{m.servidores_rec_sem_id_dele_p()}</p>
            <input class="sd-in" value={rascunho} placeholder={m.peers_identificador_placeholder()} aria-label={m.peers_identificador()}
                   autocomplete="off" autocapitalize="off" spellcheck={false} readonly={idSalvando}
                   oninput={(ev) => (editado = ev.currentTarget.value)} onkeydown={(ev) => { if (ev.key === 'Enter') salvarId(); }} />
            {#if idErro}<p class="sd-erro" role="alert">{idErro}</p>{/if}
            <div class="sd-acoes"><button type="button" class="sd-btn pri" disabled={idSalvando} onclick={salvarId}>{idSalvando ? m.config_motores_salvando() : m.ctx_salvar()}</button></div>
          {:else if card === 'sem_resposta'}
            <span class="sd-frase">{m.servidores_rec_sem_resposta({ nome })}</span>
            <p>{m.maquinas_nao_responde()}</p>
          {:else if card === 'token_recusado'}
            <span class="sd-frase">{m.servidores_rec_token_recusado({ nome })}</span>
            <p>{m.servidores_rec_token_recusado_p()}</p>
            {#if linha.navegador}<div class="sd-acoes"><button type="button" class="sd-btn pri" onclick={() => onEditar(linha.navegador!)}>{m.servidores_trocar_token()}</button></div>{/if}
          {:else if card === 'endereco_volta' && soIda}
            <span class="sd-frase">{m.servidores_rec_so_ida({ este: esteNome, nome })}</span>
            <div class="sd-acoes"><button type="button" class="sd-btn" onclick={() => (soIda = false)}>{m.servidores_rec_escolher()}</button></div>
          {:else if card === 'endereco_volta'}
            <span class="sd-frase">{m.servidores_rec_so_um_lado({ este: esteNome, nome })}</span>
            <p>{m.servidores_rec_tentou({ endereco: e.volta?.url ?? '' })}</p>
            {#if linha.navegador}{@render escolha(true)}{:else}<p>{m.maquinas_volta_sem_medir()}</p>{/if}
          {:else if card === 'volta_outra'}
            <span class="sd-frase">{m.servidores_rec_volta_outra({ este: esteNome, nome })}</span>
            <p>{m.servidores_rec_responde_como({ endereco: e.volta?.url ?? '', outro: e.volta?.identificador ?? '' })}</p>
            {#if linha.navegador}{@render escolha(false)}{/if}
          {:else if card === 'ida_outra'}
            <span class="sd-frase">{m.servidores_rec_ida_outra({ nome })}</span>
            <p>{m.maquinas_ida_outra_maquina({ nome, endereco: linha.peer?.base_url ?? '', outro: e.ida?.identificador ?? '' })}</p>
          {:else if card === 'ida_falhou'}
            <span class="sd-frase">{m.servidores_rec_ida_falhou({ este: esteNome, nome })}</span>
            <p>{m.servidores_rec_ida_falhou_p()}</p>
          {:else if card === 'falta_token'}
            <span class="sd-frase">{m.servidores_rec_falta_token({ este: esteNome, nome })}</span>
            <p>{m.servidores_rec_falta_token_p({ este: esteNome })}</p>
            <div class="sd-acoes"><button type="button" class="sd-btn pri" disabled={tokenSalvando} onclick={() => onInformarToken(linha, '')}>{tokenSalvando ? m.maquinas_add_testando() : m.servidores_usar_token_servidor({ este: esteNome })}</button></div>
            {#if tokenErro && !tokenSalvando}
              <p class="sd-erro" role="alert">{tokenErro}</p>
              <input class="sd-in" value={token} placeholder={m.sessao_token()} aria-label={m.sessao_token()}
                     autocomplete="off" autocapitalize="off" spellcheck={false}
                     oninput={(ev) => (token = ev.currentTarget.value)} onkeydown={(ev) => { if (ev.key === 'Enter') salvarToken(); }} />
              <div class="sd-acoes"><button type="button" class="sd-btn" disabled={!token.trim()} onclick={salvarToken}>{m.servidores_rec_salvar_testar()}</button></div>
            {/if}
          {:else if card === 'sem_registro'}
            <span class="sd-frase">{m.servidores_rec_sem_registro({ este: esteNome, nome })}</span>
            <p>{m.servidores_rec_sem_registro_p({ este: esteNome })}</p>
            <div class="sd-acoes"><button type="button" class="sd-btn pri" onclick={() => onFalar(linha, true)}>{m.servidores_rec_cadastrar_de_novo()}</button></div>
          {:else if card === 'pausada'}
            <span class="sd-frase">{m.servidores_desligada()}</span>
            <p>{m.maquinas_varredura_legenda()}</p>
            <div class="sd-acoes"><button type="button" class="sd-btn pri" onclick={() => onToggleScan(linha, true)}>{m.servidores_rec_religar()}</button></div>
          {/if}
          {#if e.ida?.tempo_ms != null || e.volta?.tempo_ms != null}
            <span class="sd-medida">
              {#if e.ida?.estado === 'ok'}{m.servidores_rec_medida({ de: esteNome, para: nome, ms: ms(e.ida.tempo_ms) })}{/if}
              {#if e.volta?.estado === 'ok'}&nbsp; {m.servidores_rec_medida({ de: nome, para: esteNome, ms: ms(e.volta.tempo_ms) })}{/if}
            </span>
          {/if}
          {#if ['testando', 'sem_resposta', 'ida_falhou', 'ida_outra', 'ok'].includes(card) && linha.peer}
            <div class="sd-acoes"><button type="button" class="sd-btn peq" disabled={card === 'testando'} onclick={() => onTestar(linha)}>{m.peers_testar_novamente()}</button></div>
          {/if}
        </div>
      {/if}
      {#if erro}<p class="sd-erro sd-pad sd-topo" role="alert">{erro}</p>{/if}
    </div>

    {#if linha.peer || linha.navegador}
      <details class="sd-avan">
        <summary>{m.servidores_avancado()}</summary>
        <div class="sd-cartao">
          {#if linha.navegador && linha.identificador}
            <label class="sd-campo">
              <span class="sd-rot">{m.peers_identificador()} <EscopoChip escopo="env" />
                <small>{m.maquinas_identificador_remoto_legenda({ nome })}</small>
              </span>
              <input class="sd-id" value={rascunho} placeholder={m.peers_identificador_placeholder()}
                     autocomplete="off" autocapitalize="off" spellcheck={false} readonly={idSalvando}
                     oninput={(ev) => (editado = ev.currentTarget.value)}
                     onkeydown={(ev) => { if (ev.key === 'Enter') salvarId(); }} onblur={salvarId} />
            </label>
            {#if idErro && card !== 'sem_id_dele'}<p class="sd-erro sd-pad" role="alert">{idErro}</p>{/if}
          {/if}
          {#if linha.peer}
            <label class="sd-campo">
              <span class="sd-rot">{m.maquinas_varredura()} <EscopoChip escopo="servidor" />
                <small>{m.maquinas_varredura_legenda()}</small>
              </span>
              <input type="checkbox" class="switch mq-varredura" checked={linha.peer.enabled !== false}
                     onchange={(ev) => { const alvo = ev.currentTarget; const ligar = alvo.checked; alvo.checked = !ligar; onToggleScan(linha, ligar); }} />
            </label>
            <div class="sd-campo">
              <span class="sd-rot">{m.servidores_rec_end_ida({ este: esteNome })}<small class="mq-url">{linha.peer.base_url}</small></span>
            </div>
            {#if e.volta?.url}
              <div class="sd-campo">
                <span class="sd-rot">{m.servidores_rec_end_volta({ este: esteNome })}<small class="mq-url">{e.volta.url}</small></span>
                {#if linha.navegador && !trocando}<button type="button" class="mq-editar" onclick={() => (trocando = true)}>{m.servidores_rec_trocar()}</button>{/if}
              </div>
              {#if trocando}<div class="sd-res">{@render escolha(false)}</div>{/if}
            {/if}
          {/if}
        </div>
      </details>
    {/if}

    <!-- O botão alcança os lados que ESTA linha tem: `peer` diz se o registro do servidor sai,
         `navegador` diz se há entrada aqui E se o lado de lá é alcançável. -->
    <div class="sd-rodape">
      <button type="button" class="mq-remover" aria-label={!linha.peer ? m.maquinas_remover_aria_local({ nome }) : linha.navegador ? m.maquinas_remover_aria({ nome }) : m.maquinas_remover_aria_servidor({ nome })} onclick={() => onRemover(linha)}>{m.servidores_remover_maquina()}{#if linha.peer}<EscopoChip escopo="servidor" />{/if}</button>
    </div>
  </div>
</ModalDialog>

<style>
  /* Quem rola é o miolo, não o diálogo: o vidro do ModalDialog é um ::before do tamanho da caixa. */
  :global(.modal-backdrop .modal-dialog.sd-dialogo) { width: min(520px, 94vw); max-height: 90dvh; overflow: hidden; display: flex; flex-direction: column; container-type: inline-size; }
  @media (min-width: 900px) {
    :global(.modal-backdrop .modal-dialog.sd-dialogo) { width: min(640px, 88vw); }
  }
  :global(.sd-rolagem) { position: relative; min-height: 0; overflow: auto; overscroll-behavior: contain; padding: var(--space-5); }
  .mq-linha { display: flex; flex-direction: column; }
  /* Quem rola é o miolo: sem isto o cartão (overflow: hidden) encolhe e corta o conteúdo. */
  .mq-linha > * { flex-shrink: 0; }
  .sd-cab { display: flex; align-items: flex-start; gap: var(--space-3); margin-bottom: var(--space-2); }
  .sd-titulo { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .mq-nome { margin: 0; font-size: var(--text-lg); font-weight: 600; color: var(--text-primary); display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); }
  .sd-chip { font-family: var(--font-mono); font-size: 11px; font-weight: 400; color: var(--text-muted); padding: 1px var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-full); }
  .sd-testado { font-size: var(--text-xs); color: var(--text-muted); }
  .mq-url { font-family: var(--font-mono); font-size: 12px; color: var(--text-muted); word-break: break-all; }
  .sd-fechar { width: 36px; height: 36px; min-height: 0; flex-shrink: 0; border-radius: var(--radius-sm); color: var(--text-muted); }
  .sd-fechar:hover { background: var(--bg-hover); color: var(--text-primary); }

  .sd-grupo { display: flex; align-items: center; gap: var(--space-2); margin: var(--space-4) var(--space-1) var(--space-2); font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .sd-cartao { border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; }
  .sd-campo { display: flex; align-items: center; gap: var(--space-3); min-height: 52px; padding: var(--space-2) var(--space-3); }
  .sd-cartao > * + * { border-top: 1px solid var(--border-subtle); }
  .sd-rot { flex: 1; min-width: 0; font-size: var(--text-sm); color: var(--text-primary); }
  .sd-rot small { display: block; margin-top: 2px; font-size: var(--text-xs); color: var(--text-muted); line-height: 1.35; }
  .sd-dup { margin: 0; padding: var(--space-2) var(--space-3); font-size: var(--text-xs); color: var(--warning); line-height: 1.4; }

  .sd-res { display: flex; flex-direction: column; align-items: stretch; gap: var(--space-2); padding: var(--space-3);
            background: var(--surface-inset); border-left: 3px solid var(--border-default); }
  .sd-res.ok { border-left-color: var(--success); }
  .sd-res.av { border-left-color: var(--warning); }
  .sd-res.nao { border-left-color: var(--error); }
  .sd-res p { margin: 0; font-size: var(--text-sm); line-height: 1.4; color: var(--text-secondary); }
  .sd-frase { font-size: var(--text-sm); font-weight: 600; color: var(--text-primary); }
  .sd-frase.ok { color: var(--success); }
  .sd-medida { font-size: var(--text-xs); color: var(--text-muted); }
  .sd-erro, .sd-res .sd-erro { margin: 0; font-size: var(--text-xs); color: var(--error); }
  .sd-pad { padding: 0 var(--space-3) var(--space-3); }
  .sd-topo { padding-top: var(--space-3); }
  .sd-acoes { display: flex; flex-wrap: wrap; gap: var(--space-2); }
  .sd-btn { min-height: 0; height: 36px; padding: 0 var(--space-4); border-radius: var(--radius-sm); border: 1px solid var(--border-default);
            color: var(--text-primary); font-size: var(--text-sm); }
  .sd-btn.pri { background: var(--accent); border-color: var(--accent); color: #fff; }
  .sd-btn.peq { height: 32px; padding: 0 var(--space-3); color: var(--accent); }
  .sd-btn:disabled { opacity: 0.45; }
  .sd-in { width: 100%; height: 36px; padding: 0 var(--space-3); box-sizing: border-box; background: var(--surface-inset);
           border: 1px solid var(--border-default); border-radius: var(--radius-sm); color: var(--text-primary);
           font-family: var(--font-mono); font-size: var(--text-sm); }
  .sd-opcoes { display: flex; flex-direction: column; gap: var(--space-2); }
  .sd-opcao { display: flex; gap: var(--space-2); align-items: flex-start; padding: var(--space-2); border: 1px solid var(--border-subtle);
              border-radius: var(--radius-sm); font-size: var(--text-sm); color: var(--text-primary); }
  .sd-opcao.on { border-color: var(--accent); }
  .sd-opcao input[type='radio'] { margin-top: 3px; accent-color: var(--accent); }
  .sd-opcao small { display: block; }
  .sd-outro { flex: 1; display: flex; flex-direction: column; gap: var(--space-1); }

  .sd-id { width: 9rem; flex-shrink: 0; height: 36px; padding: 0 var(--space-3); box-sizing: border-box;
           background: var(--surface-inset); border: 1px solid var(--border-default); border-radius: var(--radius-sm);
           color: var(--text-primary); font-family: var(--font-mono); font-size: var(--text-sm); }
  .mq-editar, .sd-tirar { flex-shrink: 0; min-height: 0; height: 32px; padding: 0 var(--space-3); border-radius: var(--radius-sm); border: 1px solid var(--border-default); color: var(--text-primary); font-size: var(--text-sm); }
  .sd-tirar { border: 0; color: var(--error); }
  .mq-editar:hover { background: var(--bg-hover); }

  .sd-avan summary { cursor: pointer; margin: var(--space-4) var(--space-1) var(--space-2); font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }

  .sd-rodape { display: flex; justify-content: flex-end; margin-top: var(--space-4); }
  .mq-remover { display: inline-flex; align-items: center; gap: var(--space-2); min-height: 0; height: 36px; padding: 0 var(--space-3); border-radius: var(--radius-sm); color: var(--error); font-size: var(--text-sm); }
  .mq-remover:hover { background: rgba(255, 69, 58, 0.1); }

  @container (max-width: 420px) {
    .sd-btn, .mq-editar, .sd-tirar, .mq-remover { height: 44px; }
    .sd-campo { flex-wrap: wrap; }
  }
</style>
