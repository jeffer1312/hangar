-- Linhas de aviso e erro do servidor, já limpas na máquina que mandou.
CREATE TABLE logs (
    batch_id text NOT NULL REFERENCES batches (batch_id) ON DELETE CASCADE,
    level    text NOT NULL,
    target   text NOT NULL,
    message  text NOT NULL,
    count    bigint NOT NULL CHECK (count >= 1)
);
CREATE INDEX logs_batch ON logs (batch_id);
