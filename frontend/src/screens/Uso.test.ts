// @vitest-environment happy-dom
import { expect, it, vi } from 'vitest';
import { createRawSnippet, mount, tick, unmount } from 'svelte';
import { Aquecendo, zeroUso, type UsoReport } from '@hangar/core';
import { clienteQuery } from '../lib/queries';
import * as m from '../paraglide/messages';
import Uso from './Uso.svelte';

vi.mock('../components/NavBar.svelte', () => ({ default: createRawSnippet(() => ({ render: () => '<nav></nav>' })) }));
vi.mock('../lib/queries', () => ({
  uso: (server: { id: string }, period: string, filtros: Record<string, string> = {}) => ({ id: server.id, period, conta: '', ...filtros }),
  clienteQuery: { fetchQuery: vi.fn(), invalidateQueries: vi.fn(async () => {}) },
}));

const report = (period: string): Partial<UsoReport> => ({
  totals: { ...zeroUso('totals'), sessions: 3, subagentes: 2, chamadas: 120, ctx_chars: 4000, ctx_tokens_est: 1000, input: 1_000_000 },
  by_mcp: [{ ...zeroUso('hangar'), sessions: 1, chamadas: 4 }],
  // Skills pesam pelos tokens que OCUPARAM: "muitas" ocupa mais no total; "pesada" é maior por
  // carga (15 × a mediana).
  by_skill: [
    { ...zeroUso('muitas'), plugin: 'superpowers', sessions: 2, chamadas: 10, pedidas: 2, ctx_tokens_est: 10000, ocupados_tokens_est: 100000, ocupados_eq_tokens_est: 100000, respostas: 40 },
    { ...zeroUso('pesada'), plugin: '@repo', sessions: 1, chamadas: 3, ctx_tokens_est: 4500, ocupados_tokens_est: 45000, ocupados_eq_tokens_est: 45000, respostas: 10 },
    ...Array.from({ length: 20 }, (_, i) => ({ ...zeroUso(`s${String(i).padStart(2, '0')}`), sessions: 1, chamadas: 5, ctx_tokens_est: 500, ocupados_tokens_est: 500, ocupados_eq_tokens_est: 500, respostas: 1 })),
  ],
  by_agente: [{ ...zeroUso('Explore'), sessions: 1, chamadas: 7, pedidas: 3, input: 21000,
                output: 3000, cache_read: 40000, cost: 4, cost_input: 1, cost_output: 2, cost_cache_read: 1 },
              { ...zeroUso('ecc:python-reviewer'), sessions: 1, chamadas: 2, input: 900,
                output: 500, cache_write: 8000, cache_read: 30000, cost: 9,
                cost_input: 1, cost_output: 3, cost_cache_write: 4, cost_cache_read: 1 }],
  by_bash: [{ ...zeroUso('git'), chamadas: 60 }, { ...zeroUso('grep'), chamadas: 40 }],
  by_tool: [{ ...zeroUso('Bash'), sessions: 3, chamadas: 100, ctx_chars: 4000, ctx_tokens_est: 1000 },
            { ...zeroUso('Skill'), sessions: 1, chamadas: 50 }],
  by_contexto: [{ ...zeroUso('instructions'), sessions: 2, chamadas: 2, ctx_chars: 8000, ctx_tokens_est: 2000 }],
  by_day: [{ ...zeroUso('2026-09-10'), chamadas: 120, input:130 }],
  by_conta: [{ ...zeroUso('anthropic:1'), label: 'um@x', sessions: 2, chamadas: 50 }, { ...zeroUso('anthropic:2'), label: 'dois@x', sessions: 1, chamadas: 10 }],
  applied: { period },
});
const settle = async () => { for (let i = 0; i < 12; i++) await tick(); };
const servidor = () => localStorage.setItem('cp_servers', JSON.stringify([{ id: 'a', label: 'A', baseUrl: 'https://a.test', token: 't' }]));

it('monta o painel: respostas com nome, skills por plugin, ferramentas, subagentes e a tabela com abas', async () => {
  localStorage.clear(); servidor();
  vi.mocked(clienteQuery.fetchQuery).mockImplementation((query) => {
    const { period } = query as unknown as { period: string };
    return Promise.resolve(report(period)) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  const nomes = () => [...target.querySelectorAll('table.data:not(.agentes) tr.click td.nome')].map((td) => td.firstChild?.textContent?.trim());
  const grupos = () => [...target.querySelectorAll('.grupos > li > button strong')].map((s) => s.textContent);
  try {
    await settle();
    const umDia = [...target.querySelectorAll('button')].find((b) => b.textContent?.trim() === m.custos_periodo_1d()) as HTMLButtonElement;
    umDia.click();
    await settle();
    expect(umDia.getAttribute('aria-pressed')).toBe('true');
    // Topo: cada cartão responde com um nome. "@repo" pesa menos que superpowers e não é plugin.
    const respostas = [...target.querySelectorAll('.respostas .resp')].map((r) => r.querySelector('strong')?.textContent);
    expect(respostas).toEqual(['superpowers', 'muitas', 'muitas', 'ecc', 'python-reviewer', 'Bash']);
    expect(target.querySelector('.respostas')?.textContent).toContain(m.uso_resp_ferramenta_sub({ pct: '100', n: '100' }));
    // Grupos por peso; só o primeiro abre sozinho; skill sem plugin cai em "sem plugin".
    expect(grupos()).toEqual(['superpowers', m.uso_grupo_repo(), m.uso_grupo_sem()]);
    expect([...target.querySelectorAll('.grupos button.item')].map((b) => b.querySelector('.gnome')?.textContent)).toEqual(['muitas']);
    // Ordenar por vezes: as 20 skills sem plugin (100 cargas) passam na frente.
    ([...target.querySelectorAll('.gcab .th')].find((b) => b.textContent?.includes(m.uso_col_vezes())) as HTMLButtonElement).click();
    await settle();
    expect(grupos()[0]).toBe(m.uso_grupo_sem());
    // Busca recorta as skills e o total do grupo acompanha: s00…s09 = 10 × 5 cargas.
    const busca = target.querySelector('input.busca') as HTMLInputElement;
    busca.value = 's0'; busca.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    expect(grupos()).toEqual([m.uso_grupo_sem()]);
    expect([...target.querySelectorAll('.grupos > li > button .gcel b')].map((b) => b.textContent)[1]).toBe('50');
    busca.value = ''; busca.dispatchEvent(new Event('input', { bubbles: true }));
    await settle();
    // Ferramentas sem Skill/Agent, com os comandos do Bash; subagentes agrupados pelo plugin.
    const [ferramentas] = [...target.querySelectorAll('ol.rk')];
    expect([...ferramentas.querySelectorAll('li strong')].map((s) => s.textContent)).toEqual(['Bash']);
    expect(ferramentas.textContent).toContain(m.uso_ferr_bash({ lista: 'git 60, grep 40' }));
    const agentes = target.querySelector('table.agentes')!;
    expect([...agentes.querySelectorAll('.grupo-agente strong')].map((s) => s.textContent)).toEqual(['ecc', m.uso_grupo_nativo()]);
    expect(agentes.textContent).toContain('python-reviewer');
    expect(agentes.textContent).toContain(m.custos_input_sem_cache());
    expect(agentes.textContent).toContain(m.custos_tipo_cache_lido());
    expect(agentes.textContent).toContain('30 mil');
    expect(target.textContent).not.toContain(m.uso_graf_areas());
    // Abrir outro grupo e clicar na skill abre o detalhe dela.
    ([...target.querySelectorAll('.grupos > li > button')].find((b) => b.textContent?.includes(m.uso_grupo_repo())) as HTMLButtonElement).click();
    await settle();
    ([...target.querySelectorAll('.grupos button.item')].find((b) => b.textContent?.includes('pesada')) as HTMLButtonElement).click();
    await settle();
    expect(target.querySelector('.detalhe')?.textContent).toContain('pesada');
    (target.querySelector('.detalhe button') as HTMLButtonElement).click();
    await settle();
    expect(nomes().slice(0, 2)).toEqual(['muitas', 'pesada']);                // tokens ocupados, decrescente
    // "pesada" está fora da curva por carga: leva a marca. Skill mostra cargas e respostas, não custo.
    const linhaPesada = [...target.querySelectorAll('tr.click')].find((tr) => tr.textContent?.includes('pesada'))!;
    expect(linhaPesada.querySelector('.marca')).not.toBeNull();
    expect(target.querySelector('table.data:not(.agentes) thead')?.textContent).toContain(m.uso_col_respostas());
    expect(target.querySelector('table.data:not(.agentes) thead')?.textContent).toContain(m.uso_col_cargas());
    expect(target.textContent).not.toContain('R$');
    // 22 skills: 20 visíveis + mostrar mais 2.
    expect(target.textContent).toContain(m.uso_mostrar_mais({ n: 2 }));
    // Cabeçalho reordena por tamanho da carga.
    ([...target.querySelectorAll('th .th')].find((b) => b.textContent?.includes(m.uso_col_tamanho())) as HTMLButtonElement).click();
    await settle();
    expect(nomes()[0]).toBe('pesada');
    // Aba de tools: coluna de contexto no lugar de custo.
    ([...target.querySelectorAll('[role="tab"]')].find((b) => b.textContent?.includes(m.uso_aba_tools())) as HTMLButtonElement).click();
    await settle();
    expect(nomes()).toEqual(['Bash', 'Skill']);                               // a aba mostra a tool crua
    expect(target.querySelector('table.data:not(.agentes) thead')?.textContent).toContain(m.uso_col_ctx());
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});

it('clicar numa linha abre o detalhe com série própria (foco) sem refazer o relatório principal', async () => {
  localStorage.clear(); servidor();
  const pedidos: Record<string, string>[] = [];
  vi.mocked(clienteQuery.fetchQuery).mockImplementation((query) => {
    const q = query as unknown as Record<string, string>;
    pedidos.push(q);
    return Promise.resolve(q.foco
      ? { by_day: [{ ...zeroUso('2026-09-10'), chamadas: 3, input:45 }], applied: { period: q.period } }
      : report(q.period)) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  try {
    await settle();
    const principais = pedidos.filter((p) => !p.foco).length;
    ([...target.querySelectorAll('tr.click')].find((tr) => tr.textContent?.includes('pesada')) as HTMLElement).click();
    await settle();
    expect(pedidos.filter((p) => !p.foco).length).toBe(principais);          // relatório principal intocado
    expect(pedidos.at(-1)?.foco).toBe('pesada');
    const det = target.querySelector('.detalhe')!;
    expect(det.textContent).toContain('pesada');
    expect(det.textContent).toContain(m.uso_col_ocupados());                  // skill pesa pelo que ocupou
    expect(det.querySelector('svg.serie')).not.toBeNull();
    // Números da tela continuam lá (nada foi apagado durante o detalhe).
    expect(target.querySelector('.respostas')?.textContent).toContain(m.uso_resp_plugin_skills());
    (det.querySelector('button') as HTMLButtonElement).click();
    await settle();
    expect(target.querySelector('.detalhe')).toBeNull();
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});

it('série do detalhe que falha avisa qual servidor não respondeu e tenta de novo', async () => {
  localStorage.clear(); servidor();
  let focoFalha = true;
  vi.mocked(clienteQuery.fetchQuery).mockImplementation((query) => {
    const q = query as unknown as Record<string, string>;
    if (q.foco && focoFalha) return Promise.reject(new Error('rede')) as ReturnType<typeof clienteQuery.fetchQuery>;
    return Promise.resolve(q.foco
      ? { by_day: [{ ...zeroUso('2026-09-10'), chamadas: 3, input: 45 }], applied: { period: q.period } }
      : report(q.period)) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  try {
    await settle();
    ([...target.querySelectorAll('tr.click')].find((tr) => tr.textContent?.includes('pesada')) as HTMLElement).click();
    await settle();
    const det = () => target.querySelector('.detalhe')!;
    expect(det().querySelector('.warn')?.textContent).toContain(m.custos_servidor_nao_respondeu_1());
    expect(det().querySelector('.warn')?.textContent).toContain('(A)');
    expect(det().querySelector('svg.serie')).toBeNull();
    focoFalha = false;
    ([...det().querySelectorAll('button.retry')].find((b) => b.textContent === m.config_server_tentar_de_novo()) as HTMLButtonElement).click();
    await settle();
    expect(det().querySelector('.warn')).toBeNull();
    expect(det().querySelector('svg.serie')).not.toBeNull();
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});

it('trocar filtro de conta refaz a busca com a conta e mantém o painel montado enquanto atualiza', async () => {
  localStorage.clear(); servidor();
  const pedidos: string[][] = [];
  let soltar!: () => void;
  vi.mocked(clienteQuery.fetchQuery).mockImplementation((query) => {
    const { conta, period } = query as unknown as { conta: string[] | string; period: string };
    const lista = Array.isArray(conta) ? conta : [];
    pedidos.push(lista);
    if (lista.length) return new Promise((r) => { soltar = () => r(report(period)); }) as ReturnType<typeof clienteQuery.fetchQuery>;
    return Promise.resolve(report(period)) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  try {
    await settle();
    const select = target.querySelector(`button[aria-label="${m.uso_conta()}"]`) as HTMLButtonElement;
    select.click();
    await settle();
    const opcao = (t: string) => [...document.querySelectorAll('[role="option"]')].find((b) => b.textContent?.includes(t)) as HTMLElement;
    opcao('dois@x').click();
    await settle();
    expect(pedidos.at(-1)).toEqual(['anthropic:2']);
    // Múltipla escolha: a lista continua aberta e a segunda marcação SOMA à primeira.
    expect(document.querySelector('.sel-lista')).not.toBeNull();
    opcao('um@x').click();
    await settle();
    expect(pedidos.at(-1)).toEqual(['anthropic:2', 'anthropic:1']);
    expect(select.textContent).toContain(m.uso_filtro_conta({ v: m.uso_n_de_m({ n: 2, m: 2 }) }));
    expect(target.textContent).toContain(m.uso_atualizando());
    expect(target.querySelector('.respostas')).not.toBeNull();                  // painel continua montado
    expect(target.querySelector('.esqueleto')).toBeNull();
    soltar();
    await settle();
    expect(target.textContent).not.toContain(m.uso_atualizando());
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});

it('202 "aquecendo" mostra o progresso e repergunta até o dado chegar', async () => {
  vi.useFakeTimers();
  localStorage.clear();
  localStorage.setItem('cp_servers', JSON.stringify([{ id: 'novo', label: 'Novo', baseUrl: 'https://novo.test', token: 't' }]));
  let chamadas = 0;
  vi.mocked(clienteQuery.fetchQuery).mockImplementation(() => {
    chamadas += 1;
    if (chamadas < 2) return Promise.reject(new Aquecendo(50, 200));
    return Promise.resolve(report('30d')) as ReturnType<typeof clienteQuery.fetchQuery>;
  });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  try {
    await settle();
    expect(target.textContent).toContain(m.custos_aquecendo_progresso({ maquina: 'Novo', lidos: 50, total: 200 }));
    expect(target.querySelector('.esqueleto')).not.toBeNull();
    await vi.advanceTimersByTimeAsync(3000);
    await settle();
    expect(target.querySelector('.aquecendo')).toBeNull();
    expect(target.querySelector('.respostas')?.textContent).toContain(m.uso_resp_plugin_skills());
  } finally { vi.useRealTimers(); await unmount(component); target.remove(); localStorage.clear(); }
});

it('sem uso no período mostra o vazio, não o painel', async () => {
  localStorage.clear(); servidor();
  vi.mocked(clienteQuery.fetchQuery).mockResolvedValue({ totals: zeroUso('totals'), applied: { period: '30d' } });
  const target = document.body.appendChild(document.createElement('div'));
  const component = mount(Uso, { target, props: { onBack: vi.fn() } });
  try {
    await settle();
    expect(target.textContent).toContain(m.uso_vazio());
    expect(target.querySelector('table')).toBeNull();
  } finally { await unmount(component); target.remove(); localStorage.clear(); }
});
