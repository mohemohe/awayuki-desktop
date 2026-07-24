-- application_json is already present in the canonical 003 schema. Older
-- databases are repaired by the compatibility bootstrap before their
-- migration history is baselined. Keep this tracked migration as an explicit
-- no-op so fresh databases and upgraded databases share the same history.
SELECT 1;
