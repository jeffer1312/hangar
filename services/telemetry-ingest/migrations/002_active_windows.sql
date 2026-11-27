-- "7 dias" e "30 dias" são hoje e os 6 (ou 29) dias UTC antes dele, como no resumo do painel.
CREATE OR REPLACE VIEW v_active AS
SELECT
    count(*) FILTER (WHERE last_day >= d - 6)                   AS machines_7d,
    count(*) FILTER (WHERE last_day >= d - 29)                  AS machines_30d,
    count(DISTINCT owner_id) FILTER (WHERE last_day >= d - 6)   AS owners_7d,
    count(DISTINCT owner_id) FILTER (WHERE last_day >= d - 29)  AS owners_30d
FROM v_machine_last, (SELECT (now() AT TIME ZONE 'utc')::date AS d) AS today;

CREATE OR REPLACE VIEW v_machines_per_owner AS
SELECT owner_id, count(*) AS machines, max(last_day) AS last_day
FROM v_machine_last
WHERE last_day >= (now() AT TIME ZONE 'utc')::date - 29
GROUP BY owner_id;
