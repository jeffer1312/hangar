// @vitest-environment happy-dom
// "Pastas mapeadas" (Configurações → Avançado) e o seletor NATIVO de pasta. O mecanismo já existia
// no modal de "Nova sessão" e só não tinha sido levado pra cá — a tela obrigava a DIGITAR o caminho.
// Aqui escolher no diálogo ADICIONA direto (o clique no diálogo já é a resposta), e sem shell
// Electron a tela fica exatamente como era: campo de texto + Adicionar, que é o caminho de quem usa
// pelo navegador e pelo celular.
import { describe, it, expect, vi, beforeEach, afterEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import ServerSettings from './ServerSettings.svelte';
import { criarProps } from './props-reativas.svelte';
import * as m from '../../paraglide/messages';
import type { ConfigServidorStore } from '../../lib/serverConfig.svelte';
import type { VariavelEnv } from '@hangar/core';

vi.mock('@hangar/core', async (importOriginal) => ({
  ...(await importOriginal<typeof import('@hangar/core')>()),
  listarVozesTts: vi.fn(async () => []),
  saldoTts: vi.fn(async () => ({ usados: 0, limite: 0 })),
  getPushSettings: vi.fn(() => new Promise(() => {})),
  getPushSettingsForServer: vi.fn(() => new Promise(() => {})),
  setQuietHours: vi.fn(),
  setQuietHoursForServer: vi.fn(),
}));
vi.mock('../../lib/ttsPlayer.svelte', () => ({ ttsPlayer: { tocando: false, parar: vi.fn() } }));
vi.mock('../../lib/ouvir', () => ({ ouvirAmostra: vi.fn() }));
vi.mock('../../lib/push', () => ({ enablePush: vi.fn(), pushSupported: () => true }));

/** Store de mentira com UM campo reativo: o `scan_roots` é a string "a,b" (mesmo formato do
 *  CP_SCAN_ROOTS) e o `setRascunho` a reescreve, que é o que a tela faz de verdade. `criarProps`
 *  dá o $state — sem ele o `$derived` da lista não recalcularia depois do clique. */
function criarStore(inicial: string) {
  const estado = criarProps({ valor: inicial });
  const store = {
    get campos() { return { scan_roots: { valor: estado.valor, origem: 'env' } }; },
    get leitura() { return {}; },
    get variaveisEnv() { return []; },
    get carregando() { return false; },
    get salvando() { return false; },
    get erro() { return ''; },
    get salvo() { return false; },
    get temMudanca() { return false; },
    valorAtual: (k: string) => (k === 'scan_roots' ? estado.valor : ''),
    rascunhoDe: () => '',
    setRascunho: (k: string, v: unknown) => { if (k === 'scan_roots') estado.valor = String(v); },
    carregar: vi.fn(),
    salvar: vi.fn(),
    invalidar: vi.fn(),
  } as unknown as ConfigServidorStore;
  return { store, estado };
}

function montar(inicial = '/home/voce/projetos') {
  const { store, estado } = criarStore(inicial);
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerSettings, { target: el, props: { store, secao: 'avancado' as const } });
  return { el, comp: comp as never, estado };
}

function botaoNativo(el: HTMLElement) {
  return [...el.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.criar_pasta_computador());
}

beforeEach(() => { vi.clearAllMocks(); });
afterEach(() => { delete (window as unknown as { hangar?: unknown }).hangar; });

describe('ServerSettings — Pastas mapeadas e o seletor nativo', () => {
  it('sem window.hangar (navegador/celular): sem botão, e o campo de texto continua lá', async () => {
    const t = montar();
    await tick();
    expect(botaoNativo(t.el)).toBeUndefined();
    // O caminho manual NÃO pode sumir junto — é o único de quem não roda o shell.
    expect(t.el.querySelector<HTMLInputElement>('.raiz-add input')).not.toBeNull();
    unmount(t.comp);
  });

  it('com window.hangar.pickFolder: o botão aparece e escolher ADICIONA a pasta na lista', async () => {
    const pickFolder = vi.fn().mockResolvedValue('/home/jefferson/novo-projeto');
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder };
    const t = montar('/home/voce/projetos');
    await tick();
    const btn = botaoNativo(t.el);
    expect(btn).toBeDefined();
    btn!.click();
    await tick(); await tick();
    expect(pickFolder).toHaveBeenCalledOnce();
    // Gravou no rascunho, no formato "a,b" — sem exigir um segundo clique em "Adicionar".
    expect(t.estado.valor).toBe('/home/voce/projetos,/home/jefferson/novo-projeto');
    const linhas = [...t.el.querySelectorAll('.raiz-caminho')].map((n) => n.textContent);
    expect(linhas).toContain('/home/jefferson/novo-projeto');
    unmount(t.comp);
  });

  it('cancelar o diálogo (null) não mexe na lista', async () => {
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder: vi.fn().mockResolvedValue(null) };
    const t = montar('/home/voce/projetos');
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    unmount(t.comp);
  });

  it('pasta já mapeada não duplica', async () => {
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder: vi.fn().mockResolvedValue('/home/voce/projetos') };
    const t = montar('/home/voce/projetos');
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    unmount(t.comp);
  });

  it('diálogo que falha vira erro na tela, não silêncio', async () => {
    (window as unknown as { hangar?: unknown }).hangar = {
      pickFolder: vi.fn().mockRejectedValue(new Error('dialog morreu')),
    };
    const t = montar();
    await tick();
    botaoNativo(t.el)!.click();
    await tick(); await tick();
    expect(t.el.querySelector('[role="alert"]')?.textContent).toContain('dialog morreu');
    unmount(t.comp);
  });

  it('clique duplo não abre dois diálogos concorrentes', async () => {
    // Dois abertos ao mesmo tempo resolvem fora de ordem e o último sobrescreveria o primeiro calado.
    let liberar: (v: string | null) => void = () => {};
    const pickFolder = vi.fn(() => new Promise<string | null>((res) => { liberar = res; }));
    (window as unknown as { hangar?: unknown }).hangar = { pickFolder };
    const t = montar();
    await tick();
    const btn = botaoNativo(t.el)!;
    btn.click();
    await tick();
    btn.click();
    await tick();
    expect(pickFolder).toHaveBeenCalledOnce();
    liberar(null);
    unmount(t.comp);
  });

  it('o campo de texto continua adicionando, e caminho repetido não apaga o que foi digitado', async () => {
    const t = montar('/home/voce/projetos');
    await tick();
    const form = t.el.querySelector<HTMLFormElement>('.raiz-add')!;
    const input = form.querySelector<HTMLInputElement>('input')!;
    input.value = '/home/voce/projetos';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos');
    expect(input.value).toBe('/home/voce/projetos');

    input.value = '/srv/outra';
    input.dispatchEvent(new Event('input', { bubbles: true }));
    await tick();
    form.dispatchEvent(new Event('submit', { bubbles: true, cancelable: true }));
    await tick();
    expect(t.estado.valor).toBe('/home/voce/projetos,/srv/outra');
    expect(input.value).toBe('');
    unmount(t.comp);
  });
});

describe('ServerSettings — notificações', () => {
  it('a seção Notificações traz o push e as horas silenciosas', async () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store: criarStore('').store, secao: 'notificacoes', apiTarget: null } });
    await tick();
    expect(alvo.textContent).toContain(m.notif_push_legenda());
    unmount(app);
    alvo.remove();
  });

  it('o Avançado não traz o push', async () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store: criarStore('').store, secao: 'avancado' } });
    await tick();
    expect(alvo.textContent).not.toContain(m.notif_push_legenda());
    unmount(app);
    alvo.remove();
  });
});

describe('ServerSettings — somente leitura', () => {
  it('o Avançado não repete o que mora em Máquinas', async () => {
    const store = {
      get campos() { return {}; }, get leitura() { return { port: 8765, lan_bind_ip: '0.0.0.0', server_id: 'casa', public_url: '', terminal_panel: true, versao: 'abc' }; },
      get variaveisEnv() { return []; },
      get carregando() { return false; }, get salvando() { return false; },
      get erro() { return ''; }, get salvo() { return false; }, get temMudanca() { return false; },
      valorAtual: () => '', rascunhoDe: () => '', setRascunho: vi.fn(),
      carregar: vi.fn(), salvar: vi.fn(), invalidar: vi.fn(),
    } as unknown as ConfigServidorStore;
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store, secao: 'avancado' } });
    await tick();
    expect(alvo.textContent).toContain(m.config_server_painel_terminal());
    expect(alvo.textContent).toContain('abc');
    // Cada linha só-leitura diz que só o .env a muda; as editáveis dizem "Este servidor".
    for (const linha of alvo.querySelectorAll('.ro-linha')) {
      expect(linha.textContent).toContain(m.config_escopo_env());
    }
    expect(alvo.querySelector('.linha .escopo')!.textContent).toBe(m.config_escopo_servidor());
    expect(alvo.textContent).not.toContain('8765');
    expect(alvo.textContent).not.toContain('0.0.0.0');
    expect(alvo.textContent).not.toContain('casa');
    unmount(app);
    alvo.remove();
  });

  it('"Pastas mapeadas" diz que grava no servidor, como as linhas vizinhas', async () => {
    const store = {
      get campos() { return {}; }, get leitura() { return {}; },
      get variaveisEnv() { return []; },
      get carregando() { return false; }, get salvando() { return false; },
      get erro() { return ''; }, get salvo() { return false; }, get temMudanca() { return false; },
      valorAtual: () => '/a', rascunhoDe: () => '', setRascunho: vi.fn(),
      carregar: vi.fn(), salvar: vi.fn(), invalidar: vi.fn(),
    } as unknown as ConfigServidorStore;
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store, secao: 'avancado' } });
    await tick();
    const titulo = alvo.querySelector('.raizes h3')!;
    expect(titulo.textContent).toContain(m.config_server_raizes());
    expect(titulo.textContent).toContain(m.config_escopo_servidor());
    unmount(app);
    alvo.remove();
  });

  it('sem nada em somente-leitura, o bloco "Só pelo servidor" não aparece', async () => {
    const store = {
      // Os quatro já moram em Máquinas — sem sobra nenhuma, leituraVisivel fica vazio.
      get campos() { return {}; }, get leitura() { return { port: 8765, lan_bind_ip: '0.0.0.0', server_id: 'casa', public_url: '' }; },
      get variaveisEnv() { return []; },
      get carregando() { return false; }, get salvando() { return false; },
      get erro() { return ''; }, get salvo() { return false; }, get temMudanca() { return false; },
      valorAtual: () => '', rascunhoDe: () => '', setRascunho: vi.fn(),
      carregar: vi.fn(), salvar: vi.fn(), invalidar: vi.fn(),
    } as unknown as ConfigServidorStore;
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store, secao: 'avancado' } });
    await tick();
    expect(alvo.textContent).not.toContain(m.config_server_so_servidor());
    unmount(app);
    alvo.remove();
  });
});

describe('ServerSettings — respiro do rodapé', () => {
  function storeComMudanca() {
    return {
      get campos() { return { notify_finished: { valor: true, origem: 'env' } }; },
      get leitura() { return {}; },
      get variaveisEnv() { return []; },
      get carregando() { return false; }, get salvando() { return false; },
      get erro() { return ''; }, get salvo() { return false; }, get temMudanca() { return true; },
      valorAtual: () => '', rascunhoDe: () => '', setRascunho: vi.fn(),
      carregar: vi.fn(), salvar: vi.fn(), invalidar: vi.fn(),
    } as unknown as ConfigServidorStore;
  }

  it('em Notificações o respiro vai pro bloco de push, o último renderizado', async () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store: storeComMudanca(), secao: 'notificacoes' } });
    await tick();
    const blocos = [...alvo.querySelectorAll('.cfg')];
    expect(blocos.length).toBe(2);
    expect(blocos[0].classList.contains('com-rodape')).toBe(false);
    expect(blocos[1].classList.contains('com-rodape')).toBe(true);
    unmount(app);
    alvo.remove();
  });

  it('no Avançado o respiro continua no único bloco', async () => {
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ServerSettings, { target: alvo, props: { store: storeComMudanca(), secao: 'avancado' } });
    await tick();
    const blocos = [...alvo.querySelectorAll('.cfg')];
    expect(blocos.length).toBe(1);
    expect(blocos[0].classList.contains('com-rodape')).toBe(true);
    unmount(app);
    alvo.remove();
  });
});

describe('ServerSettings — veredito de Automações', () => {
  /** O bloco de veredito que fica ABAIXO de um rótulo: é assim que a pessoa liga um ao outro. */
  function porQueDe(el: HTMLElement, rotulo: string) {
    return [...el.querySelectorAll('details.cfg-porque')].find(
      (d) => d.closest('.linha')?.textContent?.includes(rotulo),
    ) as HTMLDetailsElement | undefined;
  }

  it('Automações mostra a recomendação, e o "por quê?" traz o motivo', async () => {
    const t = montar();
    await tick();
    const d = porQueDe(t.el, m.config_server_automacoes());
    expect(d).toBeDefined();
    expect(d!.querySelector('summary')!.textContent).toContain(m.config_motores_recomendado_ligado());
    expect(d!.querySelector('summary')!.textContent).toContain(m.config_motores_por_que());
    expect(d!.querySelector('.cfg-motivo')!.textContent).toBe(m.config_server_automacoes_porque());
    // Nasce recolhido: o motivo é o que a pessoa pede, não o que ela recebe de cara.
    expect(d!.open).toBe(false);
    unmount(t.comp);
    t.el.remove();
  });

  it('linha sem veredito não ganha o bloco', async () => {
    const t = montar();
    await tick();
    expect(porQueDe(t.el, m.config_server_editor())).toBeUndefined();
    unmount(t.comp);
    t.el.remove();
  });
});

// ── Variáveis do .env (Task 7) ──────────────────────────────────────────────────────────────
// A seção é só leitura, e o que ela mostra é tudo o que a pessoa tem pra entender por que uma
// mudança não apareceu. O ponto sensível é o segredo: ele não pode chegar ao DOM por caminho nenhum.

function montarEnv(vars: VariavelEnv[], leitura: Record<string, string | number | boolean> = {}) {
  const store = {
    get campos() { return {}; },
    get leitura() { return leitura; },
    get variaveisEnv() { return vars; },
    get carregando() { return false; },
    get salvando() { return false; },
    get erro() { return ''; },
    get salvo() { return false; },
    get temMudanca() { return false; },
    valorAtual: () => '',
    rascunhoDe: () => '',
    setRascunho: vi.fn(),
    carregar: vi.fn(),
    salvar: vi.fn(),
    invalidar: vi.fn(),
  } as unknown as ConfigServidorStore;
  const el = document.createElement('div');
  document.body.appendChild(el);
  const comp = mount(ServerSettings, { target: el, props: { store, secao: 'avancado' as const } });
  return { el, comp: comp as never };
}

function variavel(over: Partial<VariavelEnv> = {}): VariavelEnv {
  return { nome: 'CP_ALGO', valor: '', definida: false, segredo: false, descricao: null, alerta: null, ...over };
}

/** A linha inteira daquela variável, texto corrido — é o que a pessoa lê. */
function linhaEnv(el: HTMLElement, nome: string): string {
  const linha = [...el.querySelectorAll('.env-linha')]
    .find((d) => d.querySelector('.env-nome')?.textContent?.trim() === nome);
  return linha?.textContent ?? '';
}

describe('ServerSettings — variáveis do .env', () => {
  it('segredo não chega ao DOM nem quando a resposta vem com valor: a linha diz só se está definida', async () => {
    // O backend não manda o valor; esta é a segunda tranca, a da tela. Sem ela, um backend velho
    // (ou com defeito) que mandasse o token o imprimiria na tela, e nenhum teste de backend pegaria.
    const t = montarEnv([
      variavel({ nome: 'CP_AUTH_TOKEN', segredo: true, definida: true, valor: 'tok-secreto-123' }),
      variavel({ nome: 'CP_DEPLOY_SECRET', segredo: true, definida: false, valor: null }),
    ]);
    await tick();
    expect(t.el.textContent).not.toContain('tok-secreto-123');
    expect(linhaEnv(t.el, 'CP_AUTH_TOKEN')).toContain(m.config_server_env_definida());
    expect(linhaEnv(t.el, 'CP_DEPLOY_SECRET')).toContain(m.config_server_env_nao_definida());
    unmount(t.comp);
    t.el.remove();
  });

  it('a descrição das lidas do ambiente aparece traduzida, ao lado do nome e do valor', async () => {
    const t = montarEnv([
      variavel({ nome: 'CP_TERMINAL', valor: 'kitty', definida: true, descricao: 'terminal' }),
      variavel({ nome: 'CP_PRICING_OFFLINE', valor: '1', definida: true, descricao: 'pricing_offline' }),
      variavel({ nome: 'CP_CLAUDE_CONFIG_DIRS', valor: 'a:/b', definida: true, descricao: 'claude_config_dirs' }),
      variavel({ nome: 'CP_ENGINES_FILE', valor: '/e.json', definida: true, descricao: 'engines_file' }),
      variavel({ nome: 'CP_CODEX_SYNC_ENABLED', valor: '1', definida: true, descricao: 'codex_sync_enabled' }),
    ]);
    await tick();
    expect(linhaEnv(t.el, 'CP_TERMINAL')).toContain(m.config_server_env_terminal());
    expect(linhaEnv(t.el, 'CP_TERMINAL')).toContain('kitty');
    expect(linhaEnv(t.el, 'CP_PRICING_OFFLINE')).toContain(m.config_server_env_pricing_offline());
    expect(linhaEnv(t.el, 'CP_CLAUDE_CONFIG_DIRS')).toContain(m.config_server_env_claude_config_dirs());
    expect(linhaEnv(t.el, 'CP_ENGINES_FILE')).toContain(m.config_server_env_engines_file());
    expect(linhaEnv(t.el, 'CP_CODEX_SYNC_ENABLED')).toContain(m.config_server_env_codex_sync_enabled());
    unmount(t.comp);
    t.el.remove();
  });

  it('a descrição dos três campos do Settings que mudam comportamento visível também aparece', async () => {
    // As outras 5 das 8 são as lidas do ambiente, no teste acima. `automations` e `codex_sync`
    // ficam de fora da lista de propósito: são editáveis pela tela, e apareceriam em duplicidade
    // com a etiqueta ".env (reinicia)", que neles é falsa.
    const t = montarEnv([
      variavel({ nome: 'CP_AUTO_RESUME', valor: false, definida: true, descricao: 'auto_resume' }),
      variavel({ nome: 'CP_OMP_PLUGIN_SYNC_ENABLED', valor: false, definida: true, descricao: 'omp_plugin_sync' }),
      variavel({ nome: 'CP_OMP_CLAUDE_CONTEXT_ENABLED', valor: true, definida: true, descricao: 'omp_claude_context' }),
    ]);
    await tick();
    expect(linhaEnv(t.el, 'CP_AUTO_RESUME')).toContain(m.config_server_env_auto_resume());
    expect(linhaEnv(t.el, 'CP_OMP_PLUGIN_SYNC_ENABLED')).toContain(m.config_server_env_omp_plugin_sync());
    expect(linhaEnv(t.el, 'CP_OMP_CLAUDE_CONTEXT_ENABLED')).toContain(m.config_server_env_omp_claude_context());
    unmount(t.comp);
    t.el.remove();
  });

  it('código de descrição que a tela não conhece mostra o nome cru, nunca o identificador', async () => {
    const t = montarEnv([variavel({ nome: 'CP_FUTURA', valor: 'x', definida: true, descricao: 'inventada_pelo_backend' })]);
    await tick();
    const linha = linhaEnv(t.el, 'CP_FUTURA');
    expect(linha).toContain('CP_FUTURA');
    expect(linha).not.toContain('inventada_pelo_backend');
    expect(t.el.querySelector('.env-desc')).toBeNull();
    unmount(t.comp);
    t.el.remove();
  });

  it('kill-switch do Codex desligado avisa que anula o botão de Harnesses', async () => {
    const t = montarEnv([variavel({
      nome: 'CP_CODEX_SYNC_ENABLED', valor: '0', definida: true,
      descricao: 'codex_sync_enabled', alerta: 'codex_sync_desligado',
    })]);
    await tick();
    expect(linhaEnv(t.el, 'CP_CODEX_SYNC_ENABLED')).toContain(m.config_server_env_alerta_codex_sync());
    unmount(t.comp);
    t.el.remove();
  });

  it('sem alerta, nenhuma linha mostra o aviso', async () => {
    // A metade negativa: provar que o aviso APARECE não prova que ele some quando não cabe, e um
    // aviso permanente diria que o botão de Harnesses está morto com ele funcionando.
    const t = montarEnv([variavel({
      nome: 'CP_CODEX_SYNC_ENABLED', valor: '1', definida: true, descricao: 'codex_sync_enabled',
    })]);
    await tick();
    expect(t.el.textContent).not.toContain(m.config_server_env_alerta_codex_sync());
    expect(t.el.querySelector('.env-alerta')).toBeNull();
    unmount(t.comp);
    t.el.remove();
  });

  it('booleano falso aparece como "não", não como vazio', async () => {
    const t = montarEnv([variavel({ nome: 'CP_AUTO_RESUME', valor: false, definida: true })]);
    await tick();
    expect(linhaEnv(t.el, 'CP_AUTO_RESUME')).toContain(m.config_server_nao());
    expect(linhaEnv(t.el, 'CP_AUTO_RESUME')).not.toContain('—');
    unmount(t.comp);
    t.el.remove();
  });

  it('servidor mais antigo, sem a lista: a seção não monta e o resto da tela fica de pé', async () => {
    const t = montarEnv([]);
    await tick();
    expect(t.el.textContent).not.toContain(m.config_server_env_titulo());
    expect(t.el.textContent).toContain(m.config_server_raizes());
    unmount(t.comp);
    t.el.remove();
  });

  it('as duas chaves só-leitura que apareciam cruas ganham rótulo', async () => {
    const t = montarEnv([], { traducao_pensamento: true, versao: 'f11610e8' });
    await tick();
    const bloco = t.el.querySelector('.somente-leitura')!;
    expect(bloco.textContent).toContain(m.config_server_traducao_pensamento());
    expect(bloco.textContent).toContain(m.config_server_versao());
    // E o nome cru da chave não sobra na tela ao lado do rótulo.
    expect(bloco.textContent).not.toContain('traducao_pensamento');
    expect(bloco.textContent).not.toContain('versao');
    unmount(t.comp);
    t.el.remove();
  });
});
