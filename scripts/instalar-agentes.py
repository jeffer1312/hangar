#!/usr/bin/env python3
"""Instala os agentes das skills do repo: link no Claude, .toml em cada home do Codex.

Uma fonte só (skills/*/agents/*.md): o Codex lê o mesmo texto em `developer_instructions`.
Idempotente. Uso: instalar-agentes.py [--home H]."""
import argparse
import json
import re
import shutil
from pathlib import Path

RAIZ = Path(__file__).resolve().parents[1]
# Pacote: o link aponta para o `current`, não para a versão (a poda a apaga).
_CURRENT = Path.home() / ".hangar" / "current"
if _CURRENT.exists() and _CURRENT.resolve() == RAIZ:
    RAIZ = _CURRENT


def cabecalho(texto: str) -> tuple[dict, str]:
    m = re.match(r"^---\n(.*?)\n---\n(.*)$", texto, re.S)
    if not m:
        return {}, texto
    campos = {}
    for linha in m.group(1).splitlines():
        k, sep, v = linha.partition(":")
        if sep:
            campos[k.strip()] = v.strip()
    return campos, m.group(2).lstrip("\n")


def toml_str(s: str) -> str:
    # json.dumps gera uma string básica TOML válida (aspas e barras escapadas, \n literal).
    return json.dumps(s, ensure_ascii=False)


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--home", default=str(Path.home()))
    home = Path(ap.parse_args().home)
    codex_homes = [p for p in home.glob(".codex*")
                   if p.is_dir() and ((p / "config.toml").exists() or (p / "agents").is_dir())]
    for md in sorted(RAIZ.glob("skills/*/agents/*.md")):
        campos, corpo = cabecalho(md.read_text(encoding="utf-8"))
        nome = campos.get("name")
        if not nome:
            continue
        destino = home / ".claude" / "agents" / f"{nome}.md"
        destino.parent.mkdir(parents=True, exist_ok=True)
        if destino.is_symlink() or destino.exists():
            destino.unlink()
        try:
            destino.symlink_to(md)
            print(f"ok: {destino} -> {md}")
        except OSError:
            # Windows sem privilégio de symlink: copia, e diz, porque um git pull não a atualiza.
            shutil.copyfile(md, destino)
            print(f"ok: {destino} (COPIA — symlink indisponivel; re-rode apos git pull)")
        toml = (f"name = {toml_str(nome)}\n"
                f"description = {toml_str(campos.get('description', ''))}\n"
                'model_reasoning_effort = "high"\n'
                f"developer_instructions = {toml_str(corpo)}\n")
        for ch in codex_homes:
            alvo = ch / "agents" / f"{nome}.toml"
            alvo.parent.mkdir(exist_ok=True)
            alvo.write_text(toml, encoding="utf-8")
            print(f"ok: {alvo}")


if __name__ == "__main__":
    main()
