\set ON_ERROR_STOP on
\getenv readonly_password YUZORA_READONLY_PASSWORD

CREATE ROLE yuzora_readonly LOGIN PASSWORD :'readonly_password';

CREATE SCHEMA alpha AUTHORIZATION yuzora_full;
CREATE SCHEMA audit AUTHORIZATION yuzora_full;

CREATE TABLE alpha.shared_name (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL
);
CREATE TABLE audit.shared_name (
  id INTEGER PRIMARY KEY,
  source TEXT NOT NULL,
  audit_only TEXT NOT NULL
);
INSERT INTO alpha.shared_name VALUES (1, 'alpha');
INSERT INTO audit.shared_name VALUES (1, 'audit', 'restricted');

CREATE TABLE alpha.value_extremes (
  id INTEGER PRIMARY KEY,
  big_value BIGINT NOT NULL,
  precise_numeric NUMERIC(38, 18) NOT NULL,
  date_value DATE NOT NULL,
  time_value TIME(6) NOT NULL,
  timestamp_value TIMESTAMP(6) NOT NULL,
  json_value JSON NOT NULL,
  binary_value BYTEA NOT NULL,
  nullable_value TEXT
);
INSERT INTO alpha.value_extremes VALUES (
  1,
  9223372036854775807,
  12345678901234567890.123456789012345678,
  DATE '2024-02-29',
  TIME '23:59:58.123456',
  TIMESTAMP '2024-02-29 12:34:56.123456',
  '{"beyondU64":18446744073709551616,"label":"fixture"}',
  decode('0001ff', 'hex'),
  NULL
);

CREATE TABLE alpha.transaction_probe (
  id INTEGER PRIMARY KEY,
  value INTEGER NOT NULL
);
INSERT INTO alpha.transaction_probe VALUES (1, 0);

CREATE TABLE alpha.query_cases (
  name TEXT PRIMARY KEY,
  sql_text TEXT NOT NULL
);
INSERT INTO alpha.query_cases VALUES
  ('syntax_error', 'SELECT 1 FROM'),
  ('long_query', 'SELECT alpha.long_query(30)'),
  ('transaction_script', 'BEGIN; SELECT value FROM alpha.transaction_probe; COMMIT;'),
  ('row_producing_dml', 'UPDATE alpha.dml_target SET touched = touched + 1 RETURNING id, touched');

CREATE TABLE alpha.dml_target (
  id INTEGER PRIMARY KEY,
  touched INTEGER NOT NULL DEFAULT 0
);
INSERT INTO alpha.dml_target(id)
SELECT value FROM generate_series(1, 1201) AS value;

CREATE TABLE audit.dml_log (
  id BIGSERIAL PRIMARY KEY,
  target_id INTEGER NOT NULL,
  touched INTEGER NOT NULL
);

CREATE FUNCTION audit.record_dml_target() RETURNS trigger
LANGUAGE plpgsql
AS $$
BEGIN
  INSERT INTO audit.dml_log(target_id, touched) VALUES (NEW.id, NEW.touched);
  RETURN NEW;
END;
$$;

CREATE TRIGGER dml_target_audit
AFTER UPDATE ON alpha.dml_target
FOR EACH ROW EXECUTE FUNCTION audit.record_dml_target();

CREATE FUNCTION alpha.long_query(delay_seconds DOUBLE PRECISION) RETURNS INTEGER
LANGUAGE SQL
AS $$ SELECT 1 FROM pg_sleep(delay_seconds) $$;

DO $fixture$
DECLARE
  row_count INTEGER;
BEGIN
  FOREACH row_count IN ARRAY ARRAY[0, 499, 500, 501, 1000, 1001, 1201]
  LOOP
    EXECUTE format('CREATE TABLE alpha.rows_%s (id INTEGER PRIMARY KEY)', row_count);
    IF row_count > 0 THEN
      EXECUTE format(
        'INSERT INTO alpha.rows_%s(id) SELECT value FROM generate_series(1, %s) AS value',
        row_count,
        row_count
      );
    END IF;
  END LOOP;
END;
$fixture$;

DO $fixture$
DECLARE
  object_index INTEGER;
BEGIN
  FOR object_index IN 0..44
  LOOP
    EXECUTE format(
      'CREATE TABLE alpha.object_%s (id INTEGER PRIMARY KEY, label TEXT NOT NULL DEFAULT %L)',
      to_char(object_index, 'FM00'),
      format('object-%s', to_char(object_index, 'FM00'))
    );
  END LOOP;
END;
$fixture$;

CREATE VIEW alpha.shared_name_view AS SELECT * FROM alpha.shared_name;

GRANT CONNECT ON DATABASE yuzora_p8 TO yuzora_readonly;
GRANT USAGE ON SCHEMA alpha TO yuzora_readonly;
GRANT SELECT ON ALL TABLES IN SCHEMA alpha TO yuzora_readonly;
GRANT EXECUTE ON FUNCTION alpha.long_query(DOUBLE PRECISION) TO yuzora_readonly;
REVOKE ALL ON SCHEMA audit FROM yuzora_readonly;
REVOKE ALL ON ALL TABLES IN SCHEMA audit FROM yuzora_readonly;

ALTER DEFAULT PRIVILEGES FOR ROLE yuzora_full IN SCHEMA alpha
GRANT SELECT ON TABLES TO yuzora_readonly;
