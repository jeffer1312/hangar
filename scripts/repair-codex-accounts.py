#!/usr/bin/env python3
"""Aplica o reparo de preparo às contas Codex já existentes."""
import asyncio
from pathlib import Path
import sys

sys.path.insert(0,str(Path(__file__).resolve().parents[1]/"backend"))
from app.codex_preparo import reparar_todas

if __name__ == "__main__":
    asyncio.run(reparar_todas())
