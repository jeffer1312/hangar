"""Descrição curta de cada entrada da configuração compartilhada, lida do próprio arquivo: o
`description` do frontmatter (skill, agent, comando), o título do .md solto, o primeiro comentário
ou docstring de um script. Nada é mantido à mão, então acompanha o que está no disco."""
import json
import re
from pathlib import Path

MAX = 220
_HEAD = 8 * 1024
_FRONT = re.compile(r"\A---\s*\n(.*?)\n---\s*(?:\n|\Z)", re.S)
_COMMENT = re.compile(r"^\s*(?:#|//|--|;|rem\s|::)\s?", re.I)


def _short(text: str) -> str:
    text = " ".join(text.split())
    if len(text) <= MAX:
        return text
    cut = text[:MAX]
    end = max(cut.rfind(". "), cut.rfind("; "))
    return cut[:end + 1] if end > MAX // 3 else cut.rstrip() + "…"


def _front_description(head: str) -> str:
    front = _FRONT.match(head)
    if not front:
        return ""
    lines = front.group(1).splitlines()
    for i, line in enumerate(lines):
        m = re.match(r"description:\s*(.*)$", line)
        if not m:
            continue
        value = m.group(1).strip()
        if value in ("", "|", ">", "|-", ">-", "|+", ">+"):
            rest = []
            for more in lines[i + 1:]:
                if more and not more[0].isspace():
                    break
                rest.append(more.strip())
            value = " ".join(rest)
        return value.strip("'\"")
    return ""


def _markdown(head: str) -> str:
    if found := _front_description(head):
        return found
    body = _FRONT.sub("", head, count=1)
    for line in body.splitlines():
        line = line.strip().lstrip("#").strip()
        if line:
            return line
    return ""


def _script(head: str) -> str:
    lines = head.splitlines()
    if lines and lines[0].startswith("#!"):
        lines = lines[1:]
    doc = re.match(r'\s*(?:[rub]*)("""|\'\'\')(.*?)(?:\1|\n\s*\n)', "\n".join(lines), re.S)
    if doc and doc.group(2).strip():
        return doc.group(2).strip().split("\n\n")[0]
    found: list[str] = []
    for line in lines:
        if "coding" in line and line.lstrip().startswith("#"):
            continue
        if _COMMENT.match(line) and not line.strip().startswith(("#!", "# shellcheck")):
            text = _COMMENT.sub("", line, count=1).strip()
            if text:
                found.append(text)
                continue
        if found or line.strip() and not line.strip().startswith(("set ", "@echo", "use ")):
            break
    return " ".join(found)


def describe(path: Path) -> str:
    """Pasta = o SKILL.md (ou README.md) dela; arquivo = frontmatter, título ou comentário."""
    try:
        if path.is_dir():
            for name in ("SKILL.md", "README.md"):
                if (path / name).is_file():
                    return describe(path / name)
            return ""
        if not path.is_file():
            return ""
        with path.open("rb") as f:
            raw = f.read(_HEAD)
        if b"\0" in raw:
            return ""
        head = raw.decode("utf-8", "replace")
    except OSError:
        return ""
    if path.suffix.lower() in (".md", ".markdown"):
        return _short(_markdown(head))
    if path.suffix.lower() in (".json", ".toml", ".yaml", ".yml", ".css", ".txt"):
        return ""
    return _short(_script(head))


def plugin_descriptions(claude: Path) -> dict[str, str]:
    """`plugin:<nome>@<marketplace>` → a descrição que o marketplace instalado dá ao plugin."""
    found: dict[str, str] = {}
    for manifest in sorted((claude / "plugins" / "marketplaces").glob("*/.claude-plugin/marketplace.json")):
        try:
            data = json.loads(manifest.read_text(encoding="utf-8"))
        except (OSError, ValueError):
            continue
        market = data.get("name") or manifest.parent.parent.name
        for plugin in data.get("plugins") or []:
            if isinstance(plugin, dict) and plugin.get("name") and plugin.get("description"):
                found[f"plugin:{plugin['name']}@{market}"] = _short(str(plugin["description"]))
    return found
