#!/usr/bin/env python3
"""Gera o jogo completo de marca do Hangar a partir de uma geometria so.

Rodar:  python3 assets/brand/gen.py        (precisa de rsvg-convert e magick)

A marca sao arcos concentricos. As pontas caem numa reta porque o angulo cai
linearmente com o raio — e o que separa "desenho em grid" de "traco a mao".

DUAS VERSOES, de proposito:
  - 3 arcos: uso normal (>= 48px).
  - 2 arcos: <= 32px. O terceiro arco empasta e vira mancha; medido reduzindo
    de verdade pra 16px, nao no olho.

COR: dentro do app a marca usa `currentColor` e adota o `--accent`, que vem da
paleta Material You do papel de parede (lib/desktopTheme.ts) — logo, muda por
maquina. Os arquivos ESTATICOS daqui nao tem como seguir isso, entao sao
monocromaticos: branco no escuro, tinta no claro. Mono nao briga com paleta
nenhuma.
"""
import math
import subprocess
import sys
from pathlib import Path

AQUI = Path(__file__).resolve().parent
CX = CY = 100.0

TINTA_ESCURA = "#100e11"   # --bg-base (dark) do app.css
TINTA_CLARA = "#f8f6f2"    # --bg-base (light)
BRANCO = "#FFFFFF"

P3 = dict(n=3, r=76, passo=23, w=13, a=30, da=12)   # 3 arcos
P2 = dict(n=2, r=74, passo=30, w=17, a=26, da=14)   # 2 arcos


def arco(r: float, a_deg: float, w: float) -> str:
    a = math.radians(a_deg)
    x1, y1 = CX + r * math.cos(math.pi - a), CY + r * math.sin(math.pi - a)
    x2, y2 = CX + r * math.cos(a), CY + r * math.sin(a)
    return (f'<path d="M {x1:.2f} {y1:.2f} A {r:.2f} {r:.2f} 0 1 1 {x2:.2f} {y2:.2f}" '
            f'fill="none" stroke="currentColor" stroke-width="{w:.2f}" stroke-linecap="round"/>')


def arcos(p: dict) -> str:
    return "\n    ".join(arco(p["r"] - i * p["passo"], p["a"] - i * p["da"], p["w"])
                         for i in range(p["n"]))


def marca(p: dict, cor: str, fundo: str | None = None, rx: int = 44,
          escala: float = 1.0, box: int = 200) -> str:
    """Marca num quadrado `box`. `escala` < 1 recua a marca (zona segura)."""
    fundo_el = f'<rect width="{box}" height="{box}" rx="{rx}" fill="{fundo}"/>\n  ' if fundo else ""
    # a marca ocupa a metade de cima do circulo: desce pra ficar no centro optico
    dy = box * 0.08 + (box - box * escala) / 2
    tx = box / 2
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {box} {box}">\n  {fundo_el}'
            f'<g color="{cor}" transform="translate({tx},{dy:.1f}) scale({escala}) '
            f'translate(-{CX},0)">\n    {arcos(p)}\n  </g>\n</svg>\n')


def lockup(cor_marca: str, cor_texto: str, fundo: str | None,
           larg: int = 1000, alt: int = 260) -> str:
    fundo_el = f'<rect width="{larg}" height="{alt}" fill="{fundo}"/>\n  ' if fundo else ""
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {larg} {alt}">\n  {fundo_el}'
            f'<g color="{cor_marca}" transform="translate(60,42) scale(0.85)">\n    {arcos(P3)}\n  </g>\n'
            f'  <text x="270" y="165" font-family="Space Grotesk, Inter, sans-serif" '
            f'font-size="120" font-weight="600" fill="{cor_texto}" letter-spacing="-2">Hangar</text>\n'
            f'</svg>\n')


def cena(nome: str, larg: int, alt: int, fundo: str, cor: str, cor_texto: str,
         escala: float, centrado: bool = True) -> str:
    """Peca larga (social/og): lockup centrado ou a esquerda sobre fundo cheio."""
    lx = (larg - 1000 * escala) / 2 if centrado else larg * 0.08
    ly = (alt - 260 * escala) / 2
    return (f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {larg} {alt}">\n'
            f'  <rect width="{larg}" height="{alt}" fill="{fundo}"/>\n'
            f'  <g transform="translate({lx:.0f},{ly:.0f}) scale({escala})">\n'
            f'    <g color="{cor}" transform="translate(60,42) scale(0.85)">\n      {arcos(P3)}\n    </g>\n'
            f'    <text x="270" y="165" font-family="Space Grotesk, Inter, sans-serif" '
            f'font-size="120" font-weight="600" fill="{cor_texto}" letter-spacing="-2">Hangar</text>\n'
            f'  </g>\n</svg>\n')


def escrever(nome: str, conteudo: str) -> Path:
    caminho = AQUI / nome
    caminho.write_text(conteudo)
    return caminho


def png(svg: str, saida: str, larg: int, alt: int | None = None) -> None:
    cmd = ["rsvg-convert", "-w", str(larg), "-h", str(alt or larg),
           str(AQUI / svg), "-o", str(AQUI / saida)]
    subprocess.run(cmd, check=True)


def main() -> None:
    # ---- fonte vetorial (o que o app importa; segue o --accent via currentColor)
    escrever("mark.svg", marca(P3, "currentColor"))
    escrever("mark-small.svg", marca(P2, "currentColor"))
    escrever("logo-lockup-dark.svg", lockup(BRANCO, "#F2F2F2", None))
    escrever("logo-lockup-light.svg", lockup(TINTA_ESCURA, TINTA_ESCURA, None))

    # ---- favicon: SVG que inverte sozinho conforme o tema do navegador
    fav = marca(P2, BRANCO, fundo=TINTA_ESCURA, rx=44).replace(
        "</svg>",
        '  <style>@media (prefers-color-scheme: light){'
        'rect{fill:#f8f6f2} g{color:#100e11}}</style>\n</svg>')
    escrever("favicon.svg", fav)

    # ---- PWA (os tamanhos que o manifest.webmanifest declara)
    escrever("_ic-512.svg", marca(P3, BRANCO, fundo=TINTA_ESCURA, rx=44))
    escrever("_ic-180.svg", marca(P3, BRANCO, fundo=TINTA_ESCURA, rx=40))
    escrever("_ic-mask.svg", marca(P3, BRANCO, fundo=TINTA_ESCURA, rx=0, escala=0.72))
    png("_ic-512.svg", "icon-512.png", 512)
    png("_ic-512.svg", "icon-192.png", 192)
    png("_ic-180.svg", "icon-180.png", 180)
    png("_ic-mask.svg", "icon-maskable-512.png", 512)

    # ---- Electron: electron-builder deriva o resto de um 512 quadrado
    png("_ic-512.svg", "electron-icon-512.png", 512)

    # ---- favicon.ico: 2 arcos, sem canto arredondado (16px nao mostra raio)
    escrever("_ico.svg", marca(P2, BRANCO, fundo=TINTA_ESCURA, rx=0))
    for s in (16, 32, 48):
        png("_ico.svg", f"_ico-{s}.png", s)
    subprocess.run(["magick"] + [str(AQUI / f"_ico-{s}.png") for s in (16, 32, 48)]
                   + [str(AQUI / "favicon.ico")], check=True)

    # ---- X/Twitter: avatar e recortado em CIRCULO -> nada de canto, marca recuada
    escrever("_avatar.svg", marca(P3, BRANCO, fundo=TINTA_ESCURA, rx=200, escala=0.80))
    png("_avatar.svg", "x-avatar-400.png", 400)
    escrever("_header.svg", cena("h", 1500, 500, TINTA_ESCURA, BRANCO, "#F2F2F2", 1.0, centrado=False))
    png("_header.svg", "x-header-1500x500.png", 1500, 500)

    # ---- GitHub: preview social do repo (a imagem do link compartilhado)
    escrever("_gh.svg", cena("g", 1280, 640, TINTA_ESCURA, BRANCO, "#F2F2F2", 0.95))
    png("_gh.svg", "github-social-1280x640.png", 1280, 640)
    png("_avatar.svg", "github-avatar-460.png", 460)

    # ---- site: og:image
    escrever("_og.svg", cena("o", 1200, 630, TINTA_ESCURA, BRANCO, "#F2F2F2", 0.95))
    png("_og.svg", "og-1200x630.png", 1200, 630)

    # ---- README: lockup rasterizado nas duas tintas
    png("logo-lockup-dark.svg", "logo-lockup-dark.png", 1000, 260)
    png("logo-lockup-light.svg", "logo-lockup-light.png", 1000, 260)

    for tmp in AQUI.glob("_*"):
        tmp.unlink()
    print("gerado em", AQUI)


if __name__ == "__main__":
    sys.exit(main())
