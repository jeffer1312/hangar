---
id: 2026-10-28-auth-library-pair
titulo: O navegador e o servidor compartilham a contagem de tentativas de acesso
---

O navegador e as outras rotas passam a usar a mesma contagem de tentativas de acesso.
Os pacotes do Hangar incluem a biblioteca de autenticação junto com o servidor e conferem
se os dois pertencem à mesma versão antes de usá-los.
A primeira atualização pelo Git também conserva esse par e a possibilidade de desfazer a troca.
