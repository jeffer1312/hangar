"""Log privado com rotação e metadados de falhas no diário compartilhável."""
import logging
import os
from logging.handlers import RotatingFileHandler

from app import diag, log_paths


class PrivateLog(RotatingFileHandler):
    def _open(self):
        fd = os.open(self.baseFilename, os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)
        return os.fdopen(fd, "a", encoding="utf-8", errors="replace")


class DiaryHandler(logging.Handler):
    def emit(self, record: logging.LogRecord) -> None:
        if record.name.startswith(("hangar.diag", "hangar.log_paths")):
            return
        # A mensagem formatada pode conter token, OAuth ou conversa. Só a origem é exportável.
        fields = {"codigo": record.name,
                  "detalhe": f"{os.path.basename(record.pathname)}:{record.lineno} {record.funcName}"}
        if record.exc_info and record.exc_info[1] is not None:
            fields.update(diag.erro_campos(record.exc_info[1]))
            frames = []
            trace = record.exc_info[2]
            while trace is not None:
                code = trace.tb_frame.f_code
                frames.append(f"{os.path.basename(code.co_filename)}:{trace.tb_lineno} {code.co_name}")
                trace = trace.tb_next
            fields["pilha"] = " <- ".join(frames[-3:])
        diag.registrar("backend.erro" if record.levelno >= logging.ERROR else "backend.aviso",
                       "erro" if record.levelno >= logging.ERROR else "aviso", **fields)


def instalar(arquivo: str = "backend.log") -> None:
    try:
        folder = log_paths.base() / "privado"
        folder.mkdir(parents=True, exist_ok=True, mode=0o700)
        loggers = [logging.getLogger(name) for name in ("hangar", "app", "uvicorn.error", "asyncio")]
        private = next((h for logger in loggers for h in logger.handlers if isinstance(h, PrivateLog)), None)
        if private is None:
            private = PrivateLog(folder / arquivo, maxBytes=4 * 1024 * 1024,
                                 backupCount=3, delay=True)
            private.setFormatter(logging.Formatter("%(asctime)s %(levelname)s [%(name)s] %(message)s"))
        for logger in loggers:
            if not any(isinstance(h, PrivateLog) for h in logger.handlers):
                logger.addHandler(private)
            if not any(isinstance(h, DiaryHandler) for h in logger.handlers):
                logger.addHandler(DiaryHandler(logging.WARNING))
        logging.getLogger("hangar").setLevel(logging.INFO)
        logging.getLogger("app").setLevel(logging.INFO)
    except OSError as exc:
        diag.registrar("diag.log_privado", "erro", **diag.erro_campos(exc))
