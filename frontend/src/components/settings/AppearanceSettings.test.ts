// @vitest-environment happy-dom
import { describe, it, expect, vi, beforeEach } from 'vitest';
import { mount, unmount, tick } from 'svelte';
import AppearanceSettings from './AppearanceSettings.svelte';
import ThemeToggle from '../ThemeToggle.svelte';
import BackgroundToggle from '../BackgroundToggle.svelte';
import * as m from '../../paraglide/messages';

// A tela inteira renderizada é a costura: o teste afirma o que está VISÍVEL e o atributo
// `disabled`, nunca classe CSS nem estado interno.
//
// Duas condições precisam ser forjadas porque a máquina que roda o teste não as reproduz: a paleta
// do papel de parede (num navegador na máquina do backend ela RESPONDE, então "Desktop apagado" só
// se prova mockando a paleta ausente) e o shell Electron.
const buscarPaleta = vi.fn(async () => null as unknown);
const paletaEmCache = vi.fn(() => null as unknown);
const isShell = vi.fn(() => false);

vi.mock('../../lib/desktopTheme', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../lib/desktopTheme')>()),
  get buscarPaleta() { return buscarPaleta; },
  get paletaEmCache() { return paletaEmCache; },
}));
vi.mock('../../lib/background', async (importOriginal) => ({
  ...(await importOriginal<typeof import('../../lib/background')>()),
  get isShell() { return isShell; },
}));

async function montar() {
  const alvo = document.createElement('div');
  document.body.appendChild(alvo);
  const app = mount(AppearanceSettings, { target: alvo });
  // Dois giros: o $effect que sonda a paleta resolve uma promessa antes de escrever no $state.
  await tick();
  await tick();
  return { alvo, app };
}

function porRotulo(alvo: HTMLElement, aria: string): HTMLButtonElement | null {
  return alvo.querySelector<HTMLButtonElement>(`[aria-label="${aria}"]`);
}

describe('AppearanceSettings — controle que não some', () => {
  beforeEach(() => {
    localStorage.clear();
    buscarPaleta.mockResolvedValue(null);
    paletaEmCache.mockReturnValue(null);
    isShell.mockReturnValue(false);
  });

  it('fora do app instalado, Fundo "Desktop" continua na fileira, desabilitado e com o motivo', async () => {
    const { alvo, app } = await montar();
    const botao = porRotulo(alvo, m.config_fundo_desktop());
    expect(botao).not.toBeNull();
    expect(botao!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_fundo_desktop());
    unmount(app);
  });

  it('sem paleta lida do servidor, Tema "Desktop" continua na fileira, desabilitado e com o motivo', async () => {
    const { alvo, app } = await montar();
    const botao = porRotulo(alvo, m.config_tema_desktop());
    expect(botao).not.toBeNull();
    expect(botao!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });

  it('com paleta lida, Tema "Desktop" fica clicável e o motivo some', async () => {
    paletaEmCache.mockReturnValue({ escuro: true, cores: {} });
    const { alvo, app } = await montar();
    const botao = porRotulo(alvo, m.config_tema_desktop());
    expect(botao!.disabled).toBe(false);
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });

  it('sem fundo de imagem nem desktop, Transparência e Solidez aparecem apagadas com o motivo', async () => {
    const { alvo, app } = await montar();
    const transparencia = alvo.querySelector<HTMLInputElement>(
      `input[aria-label="${m.config_fundo_transparencia_detalhe()}"]`,
    );
    const solidez = alvo.querySelector<HTMLInputElement>(
      `input[aria-label="${m.config_fundo_solidez_detalhe()}"]`,
    );
    expect(transparencia).not.toBeNull();
    expect(transparencia!.disabled).toBe(true);
    expect(solidez!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_fundo_imagem());
    unmount(app);
  });

  it('o motivo está ligado ao controle apagado por aria-describedby, e some junto com ele', async () => {
    const { alvo, app } = await montar();
    const botao = porRotulo(alvo, m.config_fundo_desktop())!;
    const id = botao.getAttribute('aria-describedby');
    expect(id).toBeTruthy();
    expect(alvo.querySelector(`#${id}`)!.textContent).toBe(m.config_aparencia_motivo_fundo_desktop());
    // O vínculo é condicional: a opção que funciona não carrega descrição nenhuma.
    expect(porRotulo(alvo, m.config_fundo_chapado())!.getAttribute('aria-describedby')).toBeNull();
    unmount(app);
  });

  it('com Tema ≠ Desktop, a linha "Cor do texto" fica visível com o motivo, e "Cor do tema" com o controle', async () => {
    const { alvo, app } = await montar();
    expect(alvo.textContent).toContain(m.config_aparencia_cor_texto());
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_cor_texto());
    // O inverso na mesma tela: Cor do tema está no modo editável, sem o motivo dela.
    expect(alvo.textContent).toContain(m.config_aparencia_cor_tema());
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_cor_tema());
    unmount(app);
  });

  it('com Fundo ≠ Desktop, a linha "Papel de parede do sistema" fica visível com o motivo', async () => {
    const { alvo, app } = await montar();
    expect(alvo.textContent).toContain(m.config_aparencia_papel_parede());
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_fundo_so_desktop());
    unmount(app);
  });

  it('em Leitura = Nenhum, o slider de Força fica apagado com o motivo em vez de sumir', async () => {
    const { alvo, app } = await montar();
    const nenhum = porRotulo(alvo, m.config_aparencia_nenhum_aria());
    nenhum!.click();
    await tick();
    const forca = [...alvo.querySelectorAll('label')].find((l) =>
      l.textContent?.includes(m.config_aparencia_forca()),
    );
    expect(forca).not.toBeUndefined();
    expect(forca!.querySelector('input')!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_leitura());
    // Contraste do texto só vale em Leitura = Texto ou Automática.
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_contraste());
    unmount(app);
  });

  it('em Leitura = Automática (o padrão), Força e Contraste ficam habilitados e sem motivo', async () => {
    const { alvo, app } = await montar();
    const sliders = [...alvo.querySelectorAll<HTMLInputElement>('input[type="range"]')];
    const leituraLigada = sliders.filter((i) => !i.disabled);
    expect(leituraLigada.length).toBeGreaterThan(0);
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_leitura());
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_contraste());
    unmount(app);
  });
});

// Os dois seletores são montados também FORA desta tela (folha rápida de troca de sessão, uma por
// painel do split), e três defeitos só aparecem com o componente sozinho: a sondagem em voo, a
// opção em uso e duas instâncias vivas ao mesmo tempo.
describe('ThemeToggle — a sondagem da paleta e a opção em uso', () => {
  beforeEach(() => {
    localStorage.clear();
    paletaEmCache.mockReturnValue(null);
  });

  it('enquanto a sondagem está em voo, "Desktop" fica apagada e CALADA — não afirma que falta papel de parede', async () => {
    let responder: ((v: unknown) => void) | null = null;
    buscarPaleta.mockReturnValue(new Promise((r) => { responder = r as (v: unknown) => void; }));
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ThemeToggle, { target: alvo });
    await tick();

    const botao = porRotulo(alvo, m.config_tema_desktop())!;
    expect(botao.disabled).toBe(true);
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_tema_desktop());

    // Esta máquina TEM paleta: a resposta chega e a opção abre, sem nunca ter afirmado nada.
    responder!({ escuro: true, cores: {} });
    await tick();
    await tick();
    expect(botao.disabled).toBe(false);
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });

  it('quando a sondagem volta sem paleta, aí sim "Desktop" fica apagada COM o motivo', async () => {
    buscarPaleta.mockResolvedValue(null);
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ThemeToggle, { target: alvo });
    await tick();
    await tick();
    expect(porRotulo(alvo, m.config_tema_desktop())!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });

  it('com Tema = Desktop já escolhido e a paleta fora do ar, a opção em USO não é apagada nem contradita', async () => {
    localStorage.setItem('cp_theme', 'desktop');
    buscarPaleta.mockResolvedValue(null);
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ThemeToggle, { target: alvo });
    await tick();
    await tick();
    const botao = porRotulo(alvo, m.config_tema_desktop())!;
    expect(botao.getAttribute('aria-pressed')).toBe('true');
    expect(botao.disabled).toBe(false);
    expect(alvo.textContent).not.toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });

  it('o controle do caso acima: com outro tema escolhido e a paleta fora, "Desktop" volta a ser apagada com motivo', async () => {
    localStorage.setItem('cp_theme', 'dark');
    buscarPaleta.mockResolvedValue(null);
    const alvo = document.createElement('div');
    document.body.appendChild(alvo);
    const app = mount(ThemeToggle, { target: alvo });
    await tick();
    await tick();
    expect(porRotulo(alvo, m.config_tema_desktop())!.disabled).toBe(true);
    expect(alvo.textContent).toContain(m.config_aparencia_motivo_tema_desktop());
    unmount(app);
  });
});

describe('duas instâncias vivas do mesmo seletor', () => {
  beforeEach(() => {
    localStorage.clear();
    buscarPaleta.mockResolvedValue(null);
    paletaEmCache.mockReturnValue(null);
    isShell.mockReturnValue(false);
  });

  it('cada instância tem os próprios ids, e o aria-describedby resolve dentro da própria', async () => {
    const a = document.createElement('div');
    const b = document.createElement('div');
    document.body.append(a, b);
    const apps = [
      mount(ThemeToggle, { target: a }), mount(ThemeToggle, { target: b }),
      mount(BackgroundToggle, { target: a }), mount(BackgroundToggle, { target: b }),
    ];
    await tick();
    await tick();

    for (const cont of [a, b]) {
      const descritos = [...cont.querySelectorAll('[aria-describedby]')];
      expect(descritos.length).toBeGreaterThan(0);
      for (const el of descritos) {
        const id = el.getAttribute('aria-describedby')!;
        // O alvo tem que existir DENTRO deste contêiner, e ser único no documento inteiro.
        expect(cont.querySelector(`[id="${id}"]`)).not.toBeNull();
        expect(document.querySelectorAll(`[id="${id}"]`).length).toBe(1);
      }
    }
    apps.forEach((x) => unmount(x));
  });
});
