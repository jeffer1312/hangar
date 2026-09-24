<script lang="ts">
  // Lista curta: uma linha por MÁQUINA, e a linha diz uma coisa só — se as sessões dela aparecem
  // aqui. Recados e o resto moram no detalhe. Só desenha — quem grava é MaquinasSettings.
  import * as m from '../../paraglide/messages';
  import DetalheServidor from './DetalheServidor.svelte';
  import { recolhida, sessionsState, type EstadoPeer, type LinhaMaquina, type SessionsState } from '../../lib/maquinas';
  import type { EnderecoAlcance } from '../../lib/alcance';
  import type { Server } from '../../lib/auth';

  interface Props {
    linhas: LinhaMaquina[];
    estados: Record<string, EstadoPeer>;
    meuIdentificador: string;
    esteNome: string;
    enderecos: EnderecoAlcance[];
    carregando: boolean;
    // Gravações por linha: qual está em voo ('' = nenhuma) e o erro de cada uma.
    idSalvando: string;
    idErro: Record<string, string>;
    tokenSalvando: string;
    tokenErro: Record<string, string>;
    // Erro de gravação dos recados: o detalhe aberto cobre o lugar onde a tela o mostra.
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
  }

  let { linhas, estados, meuIdentificador, esteNome, enderecos, carregando, idSalvando, idErro, tokenSalvando, tokenErro, erro = '',
        onAcompanhar, onFalar, onToggleScan, onEditar, onTirar, onInformarToken, onTestar, onUsarEndereco, onAbrirEste,
        onRemover, onSalvarIdentificador }: Props = $props();

  // Pela chave OU pelo identificador: informar o token transforma a linha "só no servidor" numa
  // linha deste aparelho (a chave muda), e o detalhe tem de continuar aberto nela.
  let aberta = $state<{ chave: string; id: string | null } | null>(null);
  const linhaAberta = $derived(aberta
    ? linhas.find((l) => l.chave === aberta!.chave) ?? (aberta.id ? linhas.find((l) => l.identificador === aberta!.id) : undefined) ?? null
    : null);

  const estadoDe = (l: LinhaMaquina) => (l.identificador ? estados[l.identificador] : undefined);
  const comEstado = $derived(linhas.map((l) => ({ l, s: sessionsState(l, estadoDe(l)) })));
  const visiveis = $derived(comEstado.filter((x) => !recolhida(x.s)));
  const fora = $derived(comEstado.filter((x) => recolhida(x.s)));

  function frase(l: LinhaMaquina, s: SessionsState): string {
    switch (s) {
      case 'aparecem': return m.servidores_sessoes_aparecem();
      case 'testando': return m.acesso_testando();
      case 'token_recusado': return m.servidores_sessoes_token_recusado();
      case 'sem_token': return m.servidores_sessoes_sem_token();
      case 'desligada': return m.servidores_desligada();
      case 'desligada_aqui': return m.servidores_sessoes_desligadas();
      case 'nao_responde': return l.navegador ? m.servidores_nao_respondeu() : m.servidores_nao_respondeu_so_recados();
    }
  }
  const farol = (s: SessionsState) => (s === 'aparecem' ? 'ok' : s === 'token_recusado' ? 'nao' : s === 'sem_token' ? 'av' : s === 'testando' ? 'test' : 'neutro');
</script>

{#snippet linhaBotao(l: LinhaMaquina, s: SessionsState)}
  {@const f = farol(s)}
  <button type="button" class="sv-linha" data-chave={l.chave}
          aria-label={m.servidores_abrir_aria({ nome: l.nome })}
          onclick={() => (aberta = { chave: l.chave, id: l.identificador })}>
    <span class="mq-farol" class:ok={f === 'ok'} class:nao={f === 'nao'} class:av={f === 'av'} aria-hidden="true">
      {f === 'test' ? '◌' : f === 'neutro' ? '·' : '●'}
    </span>
    <span class="sv-txt">
      <span class="sv-nome">{l.nome}{#if l.identificador && l.identificador !== l.nome}<span class="sv-id">{l.identificador}</span>{/if}</span>
      <span class="sv-estado" class:ok={f === 'ok'} class:nao={f === 'nao'} class:aviso={f === 'av'}>{frase(l, s)}</span>
      {#if l.navegadores.length > 1}<span class="sv-estado aviso">{m.servidores_guardado_vezes({ n: l.navegadores.length })}</span>{/if}
    </span>
    <span class="sv-chev" aria-hidden="true">›</span>
  </button>
{/snippet}

<ul class="mq-lista">
  {#each visiveis as { l, s } (l.chave)}
    <li>{@render linhaBotao(l, s)}</li>
  {:else}
    <li class="mq-vazio">{carregando ? m.comum_carregando() : fora.length ? m.servidores_nenhuma_respondendo() : m.maquinas_vazio()}</li>
  {/each}
</ul>

{#if fora.length}
  <details class="mq-fora">
    <summary>{m.servidores_nao_respondem({ n: fora.length })}</summary>
    <ul class="mq-lista">
      {#each fora as { l, s } (l.chave)}
        <li class="mq-fora-linha">
          {@render linhaBotao(l, s)}
          <button type="button" class="mq-remover" aria-label={m.servidor_remover_aria({ nome: l.nome })} onclick={() => onRemover(l)}>{m.lista_remover()}</button>
        </li>
      {/each}
    </ul>
  </details>
{/if}

{#if linhaAberta}
  <!-- Por máquina, não por chave: informar o token troca a chave da mesma máquina e o detalhe
       segue aberto; outra máquina monta um detalhe novo (e mede de novo). -->
  {#key linhaAberta.identificador ?? linhaAberta.chave}
  <DetalheServidor linha={linhaAberta} estado={estadoDe(linhaAberta)} {meuIdentificador} {esteNome} {enderecos}
    idSalvando={!!linhaAberta.navegador && idSalvando === linhaAberta.navegador.id}
    idErro={idErro[linhaAberta.navegador?.id ?? ''] ?? ''}
    tokenSalvando={tokenSalvando === linhaAberta.chave}
    tokenErro={tokenErro[linhaAberta.chave] ?? ''} {erro}
    {onAcompanhar} {onFalar} {onToggleScan} {onInformarToken} {onTestar} {onUsarEndereco} {onRemover} {onSalvarIdentificador}
    onEditar={(sv) => { aberta = null; onEditar(sv); }}
    onTirar={(sv) => { aberta = null; onTirar(sv); }}
    onAbrirEste={() => { aberta = null; onAbrirEste(); }}
    onFechar={() => (aberta = null)} />
  {/key}
{/if}

<style>
  .mq-lista { list-style: none; margin: 0; padding: 0; border: 1px solid var(--border-subtle); border-radius: var(--radius-md); overflow: hidden; }
  .mq-lista > li + li { border-top: 1px solid var(--border-subtle); }
  .sv-linha { display: flex; align-items: center; gap: var(--space-3); width: 100%; min-height: 56px; padding: var(--space-2) var(--space-3);
              text-align: left; border-radius: 0; color: var(--text-primary); }
  .sv-linha:hover { background: var(--bg-hover); }
  .mq-farol { flex-shrink: 0; width: 1.2em; text-align: center; font-size: 14px; color: var(--text-muted); }
  .mq-farol.ok { color: var(--success); }
  .mq-farol.nao { color: var(--error); }
  .mq-farol.av { color: var(--warning); }
  .sv-txt { flex: 1; min-width: 0; display: flex; flex-direction: column; gap: 2px; }
  .sv-nome { display: flex; align-items: center; flex-wrap: wrap; gap: var(--space-2); font-size: var(--text-sm); }
  .sv-id { font-family: var(--font-mono); font-size: 11px; color: var(--text-muted); padding: 1px var(--space-2); border: 1px solid var(--border-subtle); border-radius: var(--radius-full); }
  .sv-estado { font-size: var(--text-xs); color: var(--text-secondary); }
  .sv-estado.ok { color: var(--success); }
  .sv-estado.nao { color: var(--error); }
  .sv-estado.aviso { color: var(--warning); }
  .sv-chev { flex-shrink: 0; color: var(--text-muted); font-size: 18px; }
  .mq-vazio { padding: var(--space-3); font-size: var(--text-xs); color: var(--text-muted); }

  .mq-fora summary { cursor: pointer; margin: var(--space-4) 0 var(--space-2) var(--space-2); font-size: var(--text-xs); color: var(--text-muted); text-transform: uppercase; letter-spacing: 0.05em; }
  .mq-fora-linha { display: flex; align-items: center; }
  .mq-fora-linha .sv-linha { flex: 1; min-width: 0; }
  .mq-fora-linha .sv-chev { display: none; }
  .mq-remover { flex-shrink: 0; min-height: 44px; margin-right: var(--space-2); padding: 0 var(--space-3); border-radius: var(--radius-sm); color: var(--error); font-size: var(--text-sm); }
  .mq-remover:hover { background: rgba(255, 69, 58, 0.1); }
</style>
