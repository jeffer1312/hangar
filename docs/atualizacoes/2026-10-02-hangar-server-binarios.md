---
id: 2026-10-02-hangar-server-binarios
titulo: As conversas do Claude e do Codex passam a ser servidas por uma parte nova do servidor, em Rust
comando_posix: cd backend && uv run --no-sync python -m app.rust_release --never-fail
comando_windows: cd backend && .venv\Scripts\python.exe -m app.rust_release --never-fail
prova: backend/app/rust_release.py
destrutivo: true
---

O histórico e o chat ao vivo das conversas do Claude e do Codex passam a sair de um programa em
Rust que fica na frente do servidor de sempre. Sem ele (máquina sem versão publicada ou download
que falhou), o app continua funcionando como antes.
