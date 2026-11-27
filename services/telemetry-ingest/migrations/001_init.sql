CREATE TABLE batches (
    batch_id    text PRIMARY KEY,
    machine_id  text NOT NULL,
    owner_id    text NOT NULL,
    day         date NOT NULL,
    version     text NOT NULL,
    "commit"    text NOT NULL,
    os          text NOT NULL,
    arch        text NOT NULL,
    channel     text NOT NULL,
    received_at timestamptz NOT NULL DEFAULT now()
);
CREATE INDEX batches_day ON batches (day);
CREATE INDEX batches_owner ON batches (owner_id);

CREATE TABLE counters (
    batch_id text NOT NULL REFERENCES batches (batch_id) ON DELETE CASCADE,
    event    text NOT NULL,
    dims     jsonb NOT NULL DEFAULT '{}',
    bucket   text,
    count    bigint NOT NULL CHECK (count >= 0)
);
-- O apagar em cascada da retenção procura por batch_id.
CREATE INDEX counters_batch ON counters (batch_id);
CREATE INDEX counters_event ON counters (event);

-- Os dias são UTC; current_date dependeria do fuso da conexão.
-- Cada máquina com o último owner_id que mandou e o último dia visto.
CREATE VIEW v_machine_last AS
SELECT DISTINCT ON (machine_id) machine_id, owner_id, day AS last_day
FROM batches
ORDER BY machine_id, day DESC, received_at DESC;

CREATE VIEW v_active AS
SELECT
    count(*) FILTER (WHERE last_day >= d - 7)                   AS machines_7d,
    count(*) FILTER (WHERE last_day >= d - 30)                  AS machines_30d,
    count(DISTINCT owner_id) FILTER (WHERE last_day >= d - 7)   AS owners_7d,
    count(DISTINCT owner_id) FILTER (WHERE last_day >= d - 30)  AS owners_30d
FROM v_machine_last, (SELECT (now() AT TIME ZONE 'utc')::date AS d) AS today;

-- Máquinas ativas em 30 dias, agrupadas pelo último dono de cada uma.
CREATE VIEW v_machines_per_owner AS
SELECT owner_id, count(*) AS machines, max(last_day) AS last_day
FROM v_machine_last
WHERE last_day >= (now() AT TIME ZONE 'utc')::date - 30
GROUP BY owner_id;
