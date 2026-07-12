:setvar DatabaseName "yuzora_p8"

SET NOCOUNT ON;

IF DB_ID(N'$(DatabaseName)') IS NOT NULL
BEGIN
  ALTER DATABASE [$(DatabaseName)] SET SINGLE_USER WITH ROLLBACK IMMEDIATE;
  DROP DATABASE [$(DatabaseName)];
END;
GO

IF SUSER_ID(N'yuzora_full') IS NOT NULL DROP LOGIN [yuzora_full];
IF SUSER_ID(N'yuzora_readonly') IS NOT NULL DROP LOGIN [yuzora_readonly];
GO

CREATE LOGIN [yuzora_full]
  WITH PASSWORD = '$(YUZORA_FULL_PASSWORD)', CHECK_POLICY = OFF, CHECK_EXPIRATION = OFF;
CREATE LOGIN [yuzora_readonly]
  WITH PASSWORD = '$(YUZORA_READONLY_PASSWORD)', CHECK_POLICY = OFF, CHECK_EXPIRATION = OFF;
CREATE DATABASE [$(DatabaseName)];
GO

USE [$(DatabaseName)];
GO

CREATE USER [yuzora_full] FOR LOGIN [yuzora_full];
CREATE USER [yuzora_readonly] FOR LOGIN [yuzora_readonly];
GO

CREATE SCHEMA [alpha] AUTHORIZATION [yuzora_full];
GO
CREATE SCHEMA [audit] AUTHORIZATION [yuzora_full];
GO

CREATE TABLE alpha.shared_name (
  id INT NOT NULL PRIMARY KEY,
  source NVARCHAR(32) NOT NULL
);
CREATE TABLE audit.shared_name (
  id INT NOT NULL PRIMARY KEY,
  source NVARCHAR(32) NOT NULL,
  audit_only NVARCHAR(64) NOT NULL
);
INSERT INTO alpha.shared_name VALUES (1, N'alpha');
INSERT INTO audit.shared_name VALUES (1, N'audit', N'restricted');

CREATE TABLE alpha.value_extremes (
  id INT NOT NULL PRIMARY KEY,
  big_value BIGINT NOT NULL,
  precise_decimal DECIMAL(38, 18) NOT NULL,
  date_value DATE NOT NULL,
  time_value TIME(6) NOT NULL,
  timestamp_value DATETIME2(6) NOT NULL,
  json_value NVARCHAR(MAX) NOT NULL CHECK (ISJSON(json_value) = 1),
  binary_value VARBINARY(32) NOT NULL,
  nullable_value NVARCHAR(64) NULL
);
INSERT INTO alpha.value_extremes VALUES (
  1,
  9223372036854775807,
  12345678901234567890.123456789012345678,
  CONVERT(DATE, '2024-02-29'),
  CONVERT(TIME(6), '23:59:58.123456'),
  CONVERT(DATETIME2(6), '2024-02-29T12:34:56.123456'),
  N'{"beyondU64":18446744073709551616,"label":"fixture"}',
  0x0001FF,
  NULL
);

CREATE TABLE alpha.transaction_probe (
  id INT NOT NULL PRIMARY KEY,
  value INT NOT NULL
);
INSERT INTO alpha.transaction_probe VALUES (1, 0);

CREATE TABLE alpha.query_cases (
  name NVARCHAR(64) NOT NULL PRIMARY KEY,
  sql_text NVARCHAR(MAX) NOT NULL
);
INSERT INTO alpha.query_cases VALUES
  (N'syntax_error', N'SELECT 1 FROM'),
  (N'long_query', N'EXEC alpha.long_query'),
  (N'transaction_script', N'BEGIN TRANSACTION; SELECT value FROM alpha.transaction_probe; COMMIT;'),
  (N'row_producing_dml', N'DECLARE @out TABLE (id INT NOT NULL, touched INT NOT NULL); UPDATE alpha.dml_target SET touched = touched + 1 OUTPUT inserted.id, inserted.touched INTO @out; SELECT id, touched FROM @out ORDER BY id');

CREATE TABLE alpha.dml_target (
  id INT NOT NULL PRIMARY KEY,
  touched INT NOT NULL DEFAULT 0
);
;WITH rows(value) AS (
  SELECT 1
  UNION ALL
  SELECT value + 1 FROM rows WHERE value < 1201
)
INSERT INTO alpha.dml_target(id) SELECT value FROM rows OPTION (MAXRECURSION 0);

CREATE TABLE audit.dml_log (
  id BIGINT IDENTITY(1, 1) NOT NULL PRIMARY KEY,
  target_id INT NOT NULL,
  touched INT NOT NULL
);
GO

CREATE TRIGGER alpha.dml_target_audit
ON alpha.dml_target
AFTER UPDATE
AS
BEGIN
  SET NOCOUNT ON;
  INSERT INTO audit.dml_log(target_id, touched)
  SELECT id, touched FROM inserted;
END;
GO

CREATE PROCEDURE alpha.long_query
AS
BEGIN
  SET NOCOUNT ON;
  WAITFOR DELAY '00:00:30';
  SELECT CAST(1 AS INT) AS value;
END;
GO

CREATE PROCEDURE alpha.nocount_update
AS
BEGIN
  SET NOCOUNT ON;
  UPDATE alpha.transaction_probe SET value = value + 1 WHERE id = 1;
END;
GO

CREATE PROCEDURE alpha.output_update @max_id INT
AS
BEGIN
  SET NOCOUNT ON;
  DECLARE @out TABLE (id INT NOT NULL, touched INT NOT NULL);
  UPDATE alpha.dml_target
  SET touched = touched + 1
  OUTPUT inserted.id, inserted.touched INTO @out
  WHERE id <= @max_id;
  SELECT id, touched FROM @out ORDER BY id;
END;
GO

CREATE PROCEDURE alpha.affected_rows_update @max_id INT
AS
BEGIN
  SET NOCOUNT OFF;
  UPDATE alpha.dml_target SET touched = touched + 1 WHERE id <= @max_id;
END;
GO

DECLARE @counts TABLE (row_count INT NOT NULL);
INSERT INTO @counts VALUES (0), (499), (500), (501), (1000), (1001), (1201);

DECLARE @row_count INT;
DECLARE boundary_cursor CURSOR LOCAL FAST_FORWARD FOR SELECT row_count FROM @counts;
OPEN boundary_cursor;
FETCH NEXT FROM boundary_cursor INTO @row_count;
WHILE @@FETCH_STATUS = 0
BEGIN
  EXEC(N'CREATE TABLE alpha.rows_' + CONVERT(NVARCHAR(8), @row_count) +
       N' (id INT NOT NULL PRIMARY KEY)');
  IF @row_count > 0
  BEGIN
    EXEC(N';WITH rows(value) AS (
             SELECT 1
             UNION ALL
             SELECT value + 1 FROM rows WHERE value < ' + CONVERT(NVARCHAR(8), @row_count) + N'
           )
           INSERT INTO alpha.rows_' + CONVERT(NVARCHAR(8), @row_count) +
           N'(id) SELECT value FROM rows OPTION (MAXRECURSION 0)');
  END;
  FETCH NEXT FROM boundary_cursor INTO @row_count;
END;
CLOSE boundary_cursor;
DEALLOCATE boundary_cursor;

DECLARE @object_index INT = 0;
WHILE @object_index < 45
BEGIN
  DECLARE @suffix NVARCHAR(2) = RIGHT(N'0' + CONVERT(NVARCHAR(2), @object_index), 2);
  EXEC(N'CREATE TABLE alpha.object_' + @suffix +
       N' (id INT NOT NULL PRIMARY KEY, label NVARCHAR(32) NOT NULL DEFAULT N''object-' +
       @suffix + N''')');
  SET @object_index += 1;
END;
GO

CREATE VIEW alpha.shared_name_view AS SELECT id, source FROM alpha.shared_name;
GO

GRANT CONNECT TO [yuzora_full];
GRANT SELECT, INSERT, UPDATE, DELETE, EXECUTE ON SCHEMA::alpha TO [yuzora_full];
GRANT SELECT, INSERT, UPDATE, DELETE ON SCHEMA::audit TO [yuzora_full];

GRANT CONNECT TO [yuzora_readonly];
GRANT SELECT ON SCHEMA::alpha TO [yuzora_readonly];
GRANT EXECUTE ON OBJECT::alpha.long_query TO [yuzora_readonly];
DENY SELECT ON SCHEMA::audit TO [yuzora_readonly];
GO
