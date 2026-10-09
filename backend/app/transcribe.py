"""Contratos HTTP existentes, delegados ao único motor de transcrição no Rust."""
from dataclasses import dataclass
import json
from pathlib import Path

from app import transcription_bridge

DICTATION_LIMITS = (60, 150)
FILE_LIMITS = (120, 240)
VIDEO_LIMITS = (60, 120)

# A validação geral usa o mesmo limite que o motor, sem copiar o vocabulário.
_vocabulary_spec = json.loads((Path(__file__).resolve().parents[2] / "resources" / "dictation-vocabulary.json").read_text(encoding="utf-8"))
VOCAB_USUARIO_MAX = _vocabulary_spec["max_characters"] - len(_vocabulary_spec["base"]) - 2


class TranscribeError(Exception):
    def __init__(self, status: int, detail: str, code: str | None = None):
        super().__init__(detail)
        self.status, self.detail, self.code = status, detail, code

    def payload(self) -> str | dict:
        return {"code": self.code, "params": {}, "msg": self.detail} if self.code else self.detail


@dataclass(frozen=True)
class Transcription:
    text: str
    provider: str
    aviso: str | None = None


def transcribe_with_provider(content: bytes, filename: str | None,
                             limits: tuple[float, float] = DICTATION_LIMITS) -> Transcription:
    profile = "video" if limits == VIDEO_LIMITS else "file" if limits == FILE_LIMITS else "dictation"
    try:
        result = transcription_bridge.transcribe(content, filename, profile)
        return Transcription(result["text"], result["provider"], result.get("aviso"))
    except transcription_bridge.BridgeError as error:
        raise TranscribeError(error.status, error.detail, error.code) from None


def transcribe(content: bytes, filename: str | None) -> str:
    return transcribe_with_provider(content, filename, VIDEO_LIMITS).text


def providers_status() -> list[dict]:
    try:
        return transcription_bridge.providers_status()["providers"]
    except transcription_bridge.BridgeError as error:
        raise TranscribeError(error.status, error.detail, error.code) from None
