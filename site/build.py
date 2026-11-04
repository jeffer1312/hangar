#!/usr/bin/env python3
"""Gera site/public/ (PT em /, EN em /en/) a partir de src/page.html + src/strings.py."""
import html
import json
import pathlib
import re
import shutil
import sys

SITE = pathlib.Path(__file__).resolve().parent
sys.path.insert(0, str(SITE / "src"))
import strings as S  # noqa: E402

ICONS = json.loads((SITE / "src" / "icons.json").read_text(encoding="utf-8"))
OS_ICONS = json.loads((SITE / "src" / "os-icons.json").read_text(encoding="utf-8"))
OS_LOOK = {  # nome do cartão: (ícone, tom)
    "Linux": ("linux", "#F2C94C"),
    "Windows": ("windows", "#5AB0F0"),
    "macOS": ("apple", "#ECEBE6"),
    "Android": ("android", "#3DDC84"),
    "iPhone": ("apple", "#ECEBE6"),
}
HANGAR_ARCS = '<path d="M 34.18 138 A 76 76 0 1 1 165.82 138"></path><path d="M 49.59 116.38 A 53 53 0 1 1 150.41 116.38"></path><path d="M 70.16 103.14 A 30 30 0 1 1 129.84 103.14"></path>'
esc = html.escape


def lang_switch(lang):
    on = "padding: 4px 10px; border-radius: 7px; background: rgba(255,255,255,.10); color: #ECEBE6"
    off = "padding: 4px 10px; border-radius: 7px; color: #8A8C89"
    pt = f'<span aria-current="true" style="{on}">PT</span>' if lang == "pt" else f'<a href="/" style="{off}" lang="pt-BR">PT</a>'
    en = f'<span aria-current="true" style="{on}">EN</span>' if lang == "en" else f'<a href="/en/" style="{off}" lang="en">EN</a>'
    return pt + "\n" + en


def render_small(i):
    # corpo do <sc-for list="{{smallFeatures}}"> do molde, linhas 296-300, sem mudança de estilo
    return "\n".join(
        '<div class="card reveal" style="border-radius: 20px; border: 1px solid rgba(255,255,255,.08); background: #0F1112; padding: 26px; display: flex; flex-direction: column; gap: 10px">'
        f'<span style="font-family: \'Geist Mono\', monospace; font-size: 12px; color: #7A7D7B">{esc(f["k"])}</span>'
        f'<h3 style="margin: 0; font-family: \'Space Grotesk\', sans-serif; font-weight: 600; font-size: 20px; letter-spacing: -0.015em">{esc(f["t"])}</h3>'
        f'<p style="margin: 0; font-size: 15px; color: #A7A9A5">{esc(f["d"])}</p></div>'
        for f in S.SMALL[S.LANGS[i]]
    )


def render_downloads(i):
    # corpo do <sc-for list="{{downloads}}"> do molde, linhas 460-466; a borda vem de .dl/.dl.is-mine
    out = []
    for platform, os_, arch, file_, href in S.DOWNLOADS:
        icon, tone = OS_LOOK[os_[0]]
        out.append(
            f'<a class="card reveal dl" data-platform="{platform}" href="{href}" style="--t: {tone}; border-radius: 16px; background: #0F1112; padding: 22px 24px 18px; display: flex; flex-direction: column; gap: 18px">'
            f'<svg class="dl-mark" viewBox="0 0 200 200" aria-hidden="true"><g transform="translate(0,16)" fill="none" stroke="currentColor" stroke-width="13" stroke-linecap="round">{HANGAR_ARCS}</g></svg>'
            '<span class="dl-head">'
            f'<span class="dl-ic"><svg width="26" height="26" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="{OS_ICONS[icon]}"></path></svg></span>'
            '<span style="display: flex; flex-direction: column; gap: 3px; min-width: 0">'
            '<span class="dl-name">'
            f'<span style="font-family: \'Space Grotesk\', sans-serif; font-weight: 600; font-size: 22px; line-height: 1.15">{os_[i]}</span>'
            f'<span class="badge-mine" hidden style="padding: 2px 8px; border-radius: 999px; background: rgba(163,166,255,.12); color: var(--a); font-size: 11.5px; font-weight: 500; white-space: nowrap">{S.T["your_os"][i]}</span>'
            '</span>'
            f'<span style="font-size: 14px; color: #8E908D">{arch[i]}</span>'
            '</span></span>'
            '<span class="dl-foot">'
            f'<span style="min-width: 0; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; font-family: \'Geist Mono\', monospace; font-size: 12px; color: var(--a)">{file_[i]}</span>'
            '<svg class="dl-arrow" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="2" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true"><path d="M12 4v12"></path><path d="m7 11 5 5 5-5"></path><path d="M5 20h14"></path></svg>'
            '</span></a>'
        )
    return "\n".join(out)


def media(name):
    return f"/assets/media/{name}"


def render_showcase(i):
    # marcação e estilos de showcase.src.html (linhas 17-33), com os estados em classes de site.css
    tabs = []
    for k, c in enumerate(S.CLIPS):
        tabs.append(
            f'<button type="button" role="tab" class="sc-tab" aria-selected="{"true" if k == 0 else "false"}" tabindex="{0 if k == 0 else -1}" aria-controls="sc-video" '
            f'data-src="{media(c["video"][i])}" data-poster="{media(c["poster"][i])}" data-caption="{esc(c["caption"][i])}" data-alt="{esc(c["alt"][i])}" '
            'style="position: relative; flex: none; height: 38px; padding: 0 16px; border: 0; border-radius: 9px; font: inherit; font-size: 13.5px; cursor: pointer; overflow: hidden; transition: background .3s, color .3s">'
            f'{esc(c["tab"][i])}'
            '<span class="sc-track" aria-hidden="true" style="position: absolute; left: 12px; right: 12px; bottom: 3px; height: 2px; border-radius: 1px; background: rgba(255,255,255,.08)">'
            '<span class="sc-bar" style="display: block; height: 100%; width: 0%; border-radius: 1px; background: var(--a); transition: width .25s linear"></span></span></button>'
        )
    first = S.CLIPS[0]
    return (
        '<div class="showcase" data-showcase style="display: flex; flex-direction: column; gap: 18px">'
        f'<div role="tablist" aria-label="{S.T["sc_tabs"][i]}" style="align-self: center; max-width: 100%; overflow-x: auto; display: flex; gap: 2px; padding: 4px; border-radius: 12px; border: 1px solid rgba(255,255,255,.09); background: rgba(255,255,255,.02)">{"".join(tabs)}</div>'
        '<div class="tilt" style="border-radius: 16px; padding: 1px; background: linear-gradient(180deg, rgba(255,255,255,.24), rgba(255,255,255,.05) 40%, rgba(255,255,255,0)); box-shadow: 0 40px 120px -30px rgba(0,0,0,.9)">'
        f'<div class="sc-frame"><video id="sc-video" class="sc-video" muted playsinline preload="auto" poster="{media(first["poster"][i])}" src="{media(first["video"][i])}" aria-label="{esc(first["alt"][i])}"></video></div></div>'
        f'<p class="sc-caption" aria-live="polite" style="margin: 0; text-align: center; font-size: 15px; color: #A3A5A2; min-height: 1.6em">{esc(first["caption"][i])}</p></div>'
    )


def render_page(i):
    lang = S.LANGS[i]
    page = (SITE / "src" / "page.html").read_text(encoding="utf-8")
    words = S.T["h1"][i].split(" ")
    clip = {c["key"]: c for c in S.CLIPS}
    extra = {
        "canonical": S.BASE_URL + ("/" if lang == "pt" else "/en/"),
        "h1_plain": S.T["h1"][i],
        "h1_words": " ".join(f'<span class="w" style="animation-delay: {0.1 + j * 0.07:.2f}s">{w}</span>' for j, w in enumerate(words)),
        "lang_switch": lang_switch(lang),
        "SMALL": render_small(i),
        "DOWNLOADS": render_downloads(i),
        "SHOWCASE": render_showcase(i),
        "VIDEO_orq": media(clip["orq"]["video"][i]), "POSTER_orq": media(clip["orq"]["poster"][i]),
        "VIDEO_pair": media(clip["pair"]["video"][i]), "POSTER_pair": media(clip["pair"]["poster"][i]),
    }
    for k, paths in ICONS.items():
        extra[f"ICON_{k}"] = "".join(p.replace("/>", "></path>") for p in paths)
    for key, value in {**{k: v[i] for k, v in S.T.items()}, **extra}.items():
        page = page.replace(f"«{key}»", value)
    left = sorted(set(re.findall(r"«[^»]*»", page)))
    if left:
        raise SystemExit(f"marcador sem texto em {lang}: {left}")
    return page


def build(out: pathlib.Path) -> list[pathlib.Path]:
    if out.exists():
        shutil.rmtree(out)
    (out / "en").mkdir(parents=True)
    shutil.copytree(SITE / "media", out / "assets" / "media")
    shutil.copy(SITE / "src" / "site.css", out / "assets" / "site.css")
    shutil.copy(SITE / "src" / "site.js", out / "assets" / "site.js")
    for f in (SITE / "static").iterdir():
        shutil.copy(f, out / f.name)
    pages = []
    for i, lang in enumerate(S.LANGS):
        dest = out / ("index.html" if lang == "pt" else "en/index.html")
        dest.write_text(render_page(i), encoding="utf-8")
        pages.append(dest)
    (out / "sitemap.xml").write_text(
        '<?xml version="1.0" encoding="UTF-8"?>\n<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">'
        f"<url><loc>{S.BASE_URL}/</loc></url><url><loc>{S.BASE_URL}/en/</loc></url></urlset>\n",
        encoding="utf-8",
    )
    # referência local quebrada vira 404 no ar; o deploy.sh roda o build, então falha aqui
    missing = []
    for page in pages:
        for ref in re.findall(r'(?:src|href|poster|data-src|data-poster)="(/[^"#]*)"', page.read_text(encoding="utf-8")):
            target = out / ref.lstrip("/")
            if ref.endswith("/"):
                target = target / "index.html"
            if not target.exists():
                missing.append(f"{page.relative_to(out).as_posix()}: {ref}")
    if missing:
        raise SystemExit("referência local sem arquivo:\n" + "\n".join(missing))
    return pages


if __name__ == "__main__":
    for p in build(SITE / "public"):
        print(p.relative_to(SITE))
