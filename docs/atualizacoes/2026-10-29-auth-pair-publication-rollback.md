---
id: 2026-10-29-auth-pair-publication-rollback
titulo: A preparação do servidor conserva o par anterior quando falha
---

Uma falha ao preparar o servidor e sua biblioteca de autenticação tenta conservar os arquivos
anteriores. Se o sistema também impedir o desfazer, o erro informa a causa e onde ficou a cópia
para recuperação, em vez de esconder a falha.
