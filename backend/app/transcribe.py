"""Limite de configuração do vocabulário compartilhado com o motor Rust."""
import json
from pathlib import Path

# A validação geral usa o mesmo limite que o motor, sem copiar o vocabulário.
_vocabulary_spec = json.loads((Path(__file__).resolve().parents[2] / "resources" / "dictation-vocabulary.json").read_text(encoding="utf-8"))
VOCAB_USUARIO_MAX = _vocabulary_spec["max_characters"] - len(_vocabulary_spec["base"]) - 2
