-- De onde o binário subiu, a diferença do relógio da máquina (chegada menos envio, nula sem a hora de envio)
-- e o ambiente fechado da máquina no envio.
ALTER TABLE batches ADD COLUMN install text, ADD COLUMN clock_skew_s integer, ADD COLUMN machine jsonb NOT NULL DEFAULT '{}';

-- O retrato da primeira e da última vez, os passos antes da última, o que veio depois e se a contagem bateu no teto.
ALTER TABLE records ADD COLUMN ctx_first jsonb, ADD COLUMN ctx_last jsonb, ADD COLUMN trail_last jsonb NOT NULL DEFAULT '[]',
    ADD COLUMN after text, ADD COLUMN capped boolean NOT NULL DEFAULT false;
ALTER TABLE counters ADD COLUMN capped boolean NOT NULL DEFAULT false;
ALTER TABLE trail_steps ADD COLUMN capped boolean NOT NULL DEFAULT false;
