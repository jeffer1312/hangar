import os
import shutil
import subprocess
from pathlib import Path

# Vídeo anexado pelo celular chegava como um caminho que o modelo NÃO consegue abrir: o Read não lê
# mp4. O player do modal servia pra você ver, e só. Aqui o vídeo vira coisa legível — quadros
# distribuídos ao longo da duração + o áudio extraído pra transcrição.
#
# Tudo é best-effort: sem ffmpeg, vídeo corrompido ou sem trilha de áudio, devolve vazio e o upload
# segue normal com o caminho do vídeo. Anexo é do usuário; enriquecer não pode custar o anexo.

VIDEO_EXTS = {"mp4", "mov", "webm", "mkv", "m4v", "avi"}
FRAMES = 6            # o bastante pra dar o arco do vídeo sem inflar o custo de visão
# Os timeouts são POR chamada e o extract_frames faz uma por quadro, tudo dentro de um to_thread do
# pool compartilhado do app — teto generoso aqui vira fila travando endpoint sem relação. Com `-ss`
# antes do `-i` o seek é direto, então um quadro leva ~1s mesmo em arquivo grande; 25s já é folga
# enorme. O áudio percorre o arquivo inteiro, por isso tem mais prazo.
_TIMEOUT_QUADRO = 25
_TIMEOUT_AUDIO = 90


def is_video(path: str) -> bool:
    return Path(path).suffix.lower().lstrip(".") in VIDEO_EXTS


def _ffmpeg() -> str | None:
    return shutil.which("ffmpeg")


def duration_s(path: str) -> float:
    """Duração em segundos (0.0 se não der pra ler)."""
    probe = shutil.which("ffprobe")
    if not probe:
        return 0.0
    try:
        out = subprocess.run(
            [probe, "-v", "error", "-show_entries", "format=duration",
             "-of", "default=noprint_wrappers=1:nokey=1", path],
            capture_output=True, text=True, timeout=30,
        )
        return float((out.stdout or "0").strip() or 0)
    except (subprocess.SubprocessError, ValueError, OSError):
        return 0.0


def extract_frames(path: str, n: int = FRAMES) -> list[str]:
    """N quadros distribuídos pela duração, gravados ao lado do vídeo. Devolve os caminhos.

    Distribuir pela duração (em vez de pegar os N primeiros segundos) é o que faz os quadros
    contarem o vídeo: um clipe de 3 minutos com os 6 primeiros quadros não mostra nada.
    """
    ff = _ffmpeg()
    if not ff or n <= 0:
        return []
    dur = duration_s(path)
    src = Path(path)
    saidas: list[str] = []
    # Sem duração legível, cai pra 1 quadro no começo — melhor um do que nenhum.
    marcas = [dur * (i + 0.5) / n for i in range(n)] if dur > 0 else [0.0]
    for i, t in enumerate(marcas):
        dest = src.with_name(f"{src.stem}-q{i + 1}.jpg")
        try:
            r = subprocess.run(
                [ff, "-nostdin", "-y", "-ss", f"{t:.2f}", "-i", str(src),
                 "-frames:v", "1", "-vf", "scale='min(1568,iw)':-2", "-q:v", "4", str(dest)],
                capture_output=True, timeout=_TIMEOUT_QUADRO,
            )
        except (subprocess.SubprocessError, OSError):
            continue
        if r.returncode == 0 and dest.is_file() and dest.stat().st_size > 0:
            saidas.append(os.path.realpath(dest))
    return saidas


def extract_audio(path: str) -> str | None:
    """Trilha de áudio em m4a ao lado do vídeo, pra transcrever. None se não houver áudio."""
    ff = _ffmpeg()
    if not ff:
        return None
    src = Path(path)
    dest = src.with_name(f"{src.stem}-audio.m4a")
    try:
        r = subprocess.run(
            [ff, "-nostdin", "-y", "-i", str(src), "-vn", "-ac", "1", "-ar", "16000",
             "-c:a", "aac", "-b:a", "64k", str(dest)],
            capture_output=True, timeout=_TIMEOUT_AUDIO,
        )
    except (subprocess.SubprocessError, OSError):
        return None
    if r.returncode != 0 or not dest.is_file() or dest.stat().st_size == 0:
        return None
    return os.path.realpath(dest)
