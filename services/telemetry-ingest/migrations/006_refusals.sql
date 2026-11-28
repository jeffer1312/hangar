-- O "Não enviar" de cada máquina, uma vez por máquina e sem identificação: só a contagem por dia.
CREATE TABLE refusals (
    day     date NOT NULL DEFAULT (now() AT TIME ZONE 'utc')::date,
    kind    text NOT NULL CHECK (kind IN ('declined', 'revoked')),
    version text NOT NULL,
    os      text NOT NULL,
    channel text NOT NULL,
    count   bigint NOT NULL DEFAULT 1 CHECK (count >= 1),
    PRIMARY KEY (day, kind, version, os, channel)
);
