-- Retrato do dia ainda aberto: o mesmo batch_id volta de hora em hora e troca o que tinha.
ALTER TABLE batches ADD COLUMN partial boolean NOT NULL DEFAULT false;

-- Falhas e avisos do diário de uso, com os campos já limpos na máquina.
CREATE TABLE records (
    batch_id text NOT NULL REFERENCES batches (batch_id) ON DELETE CASCADE,
    level    text NOT NULL,
    event    text NOT NULL,
    count    bigint NOT NULL CHECK (count >= 1),
    first_at time NOT NULL,
    last_at  time NOT NULL,
    fields   jsonb NOT NULL DEFAULT '{}',
    trail    jsonb NOT NULL DEFAULT '[]'
);
CREATE INDEX records_batch ON records (batch_id);

-- Cada passo do dia pelo nome, na ordem em que aconteceu.
CREATE TABLE trail_steps (
    batch_id text NOT NULL REFERENCES batches (batch_id) ON DELETE CASCADE,
    seq      int NOT NULL,
    t        time NOT NULL,
    event    text NOT NULL,
    level    text NOT NULL,
    screen   text,
    count    bigint NOT NULL CHECK (count >= 1),
    PRIMARY KEY (batch_id, seq)
);
