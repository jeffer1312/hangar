-- Contas do painel: e-mail e senha (argon2), sessões e convites de uso único.
-- Tokens de sessão e de convite ficam só como sha256: o banco vazado não abre sessão nem convite.
CREATE TABLE panel_users (
    id            bigserial PRIMARY KEY,
    email         text NOT NULL UNIQUE,
    password_hash text NOT NULL,
    created_at    timestamptz NOT NULL DEFAULT now()
);

CREATE TABLE panel_sessions (
    token_hash text PRIMARY KEY,
    user_id    bigint NOT NULL REFERENCES panel_users (id) ON DELETE CASCADE,
    expires_at timestamptz NOT NULL
);
CREATE INDEX panel_sessions_user ON panel_sessions (user_id);

CREATE TABLE panel_invites (
    token_hash text PRIMARY KEY,
    created_by bigint REFERENCES panel_users (id) ON DELETE SET NULL,
    expires_at timestamptz NOT NULL,
    used_at    timestamptz,
    used_by    bigint REFERENCES panel_users (id) ON DELETE SET NULL
);
