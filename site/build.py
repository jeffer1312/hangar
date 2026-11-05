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
OS_ICON = {"linux": "linux", "windows": "windows", "macos": "apple", "android": "android", "iphone": "apple"}
HANGAR_ARCS = '<path d="M 34.18 138 A 76 76 0 1 1 165.82 138"></path><path d="M 49.59 116.38 A 53 53 0 1 1 150.41 116.38"></path><path d="M 70.16 103.14 A 30 30 0 1 1 129.84 103.14"></path>'
DOWNLOAD_ARROW = '<path d="M12 4v12"></path><path d="m7 11 5 5 5-5"></path><path d="M5 20h14"></path>'
GO_ARROW = '<path d="M5 12h14"></path><path d="m13 6 6 6-6 6"></path>'
OWN_ICONS = {
    "pi": '<svg viewBox="0 0 560 560" fill="currentColor" aria-hidden="true"><path d="M420 280H280V140H0V0H420V280Z"></path><path d="M560 560H420V280H560V560Z"></path><path d="M140 560H0V140H140V280H280V420H140V560Z"></path></svg>',
    "omp": '<svg viewBox="0 0 120 90" fill="currentColor" aria-hidden="true"><rect x="10" y="8" width="100" height="12" rx="2"></rect><rect x="25" y="20" width="12" height="62" rx="2"></rect><rect x="75" y="20" width="12" height="45" rx="2"></rect><rect x="71" y="55" width="20" height="16" rx="3"></rect></svg>',
}
esc = html.escape


def agent_icon(key):
    if key in OWN_ICONS:
        return OWN_ICONS[key]
    paths = "".join(p.replace("/>", "></path>") for p in ICONS[key])
    return f'<svg viewBox="0 0 24 24" fill="currentColor" aria-hidden="true">{paths}</svg>'


def render_agents_lead(i):
    names = [f'<span class="ag"><span class="ag-i ag-{key}">{agent_icon(key)}</span>{name}</span>' for key, name in S.AGENTS]
    return ", ".join(names[:-1]) + S.T["agents_and"][i] + names[-1]


def render_flags():
    return "".join(f'<li class="ag-{key}" style="--i:{n}">{agent_icon(key)}<span>{name}</span></li>' for n, (key, name) in enumerate(S.AGENTS))


def lang_switch(lang):
    pt = '<span aria-current="true">PT</span>' if lang == "pt" else '<a href="/" lang="pt-BR">PT</a>'
    en = '<span aria-current="true">EN</span>' if lang == "en" else '<a href="/en/" lang="en">EN</a>'
    return pt + "\n" + en


def render_small(i):
    return "\n".join(
        f'<div class="mini"><span class="mono">{esc(f["k"])}</span><h3>{esc(f["t"])}</h3><p>{esc(f["d"])}</p></div>'
        for f in S.SMALL[S.LANGS[i]]
    )


def render_downloads(i):
    out = []
    for platform, os_, arch, file_, href in S.DOWNLOADS:
        out.append(
            f'<a class="dl" data-platform="{platform}" href="{href}">'
            '<span class="dl-head">'
            f'<svg width="24" height="24" viewBox="0 0 24 24" fill="currentColor" aria-hidden="true"><path d="{OS_ICONS[OS_ICON[platform]]}"></path></svg>'
            '<span class="dl-info">'
            f'<span class="dl-name">{os_[i]}<span class="badge-mine" hidden>{S.T["your_os"][i]}</span></span>'
            f'<span class="dl-arch">{arch[i]}</span>'
            '</span></span>'
            '<span class="dl-foot">'
            f'<span class="dl-file mono">{file_[i]}</span>'
            f'<svg class="dl-arrow" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" aria-hidden="true">{GO_ARROW if href.startswith("#") else DOWNLOAD_ARROW}</svg>'
            '</span></a>'
        )
    return "\n".join(out)


def media(name):
    return f"/assets/media/{name}"


def render_showcase(i):
    tabs = []
    for k, c in enumerate(S.CLIPS):
        tabs.append(
            f'<button type="button" role="tab" class="sc-tab" aria-selected="{"true" if k == 0 else "false"}" tabindex="{0 if k == 0 else -1}" aria-controls="sc-video" '
            f'data-src="{media(c["video"][i])}" data-poster="{media(c["poster"][i])}" data-caption="{esc(c["caption"][i])}" data-alt="{esc(c["alt"][i])}">'
            f'{esc(c["tab"][i])}'
            '<span class="sc-track" aria-hidden="true"><span class="sc-bar"></span></span></button>'
        )
    first = S.CLIPS[0]
    return (
        '<div class="showcase" data-showcase>'
        f'<div class="sc-window"><div class="sc-frame"><video id="sc-video" class="sc-video" muted playsinline preload="auto" poster="{media(first["poster"][i])}" src="{media(first["video"][i])}" aria-label="{esc(first["alt"][i])}"></video></div></div>'
        f'<div class="sc-tabs" role="tablist" aria-label="{S.T["sc_tabs"][i]}">{"".join(tabs)}</div>'
        f'<p class="sc-caption" aria-live="polite">{esc(first["caption"][i])}</p></div>'
    )


def render_page(i):
    lang = S.LANGS[i]
    page = (SITE / "src" / "page.html").read_text(encoding="utf-8")
    clip = {c["key"]: c for c in S.CLIPS}
    extra = {
        "canonical": S.BASE_URL + ("/" if lang == "pt" else "/en/"),
        "og_image": S.BASE_URL + media(f"og-card-{lang}.png"),
        "h1_plain": S.T["h1"][i],
        "lang_switch": lang_switch(lang),
        "SMALL": render_small(i),
        "DOWNLOADS": render_downloads(i),
        "HANGAR_ARCS": HANGAR_ARCS,
        "SHOWCASE": render_showcase(i),
        "AGENTS_LEAD": render_agents_lead(i),
        "FLAGS": render_flags(),
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
