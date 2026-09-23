use yuzora_lib::db_service::integration_harness::{IntegrationConnection, IntegrationRuntime};
use yuzora_lib::db_service::{DbValue, StatementExecutionResult};

async fn execute(connection: &IntegrationConnection, sql: String) -> Option<String> {
    let run = connection
        .run_primary(uuid::Uuid::new_v4().to_string(), sql)
        .await
        .unwrap();
    assert!(!run.transaction_may_be_open);
    assert!(!matches!(
        run.statements[0].effect_outcome,
        yuzora_lib::db_service::EffectOutcome::RolledBack
            | yuzora_lib::db_service::EffectOutcome::TransactionPending
    ));
    match &run.statements[0].result {
        StatementExecutionResult::Execute { affected_rows } => affected_rows.clone(),
        result => panic!("expected a completed write: {result:?}"),
    }
}

#[tokio::test]
#[ignore = "uses isolated database fixtures; select engines with YUZORA_DATABASE_TEST_ENGINES"]
async fn catalog_discovery_and_table_editing() {
    std::env::set_var(
        yuzora_lib::db_query_worker::WORKER_BIN_ENV,
        env!("CARGO_BIN_EXE_yuzora"),
    );
    let engines = std::env::var("YUZORA_DATABASE_TEST_ENGINES").unwrap_or_else(|_| "sqlite".into());
    let password = std::env::var("YUZORA_P8_DATABASE_PASSWORD").unwrap_or_default();
    for engine in engines.split(',') {
        let runtime = IntegrationRuntime::default();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("workbench.sqlite");
        let connection = match engine {
            "sqlite" => {
                drop(rusqlite::Connection::open(&path).unwrap());
                runtime
                    .open_sqlite("workbench", path.to_string_lossy())
                    .await
                    .unwrap()
            }
            "postgres" => {
                let discovery = runtime
                    .open_postgres(
                        "discovery",
                        "127.0.0.1",
                        55432,
                        "",
                        "yuzora_full",
                        password.clone(),
                        true,
                        true,
                    )
                    .await
                    .unwrap();
                assert!(discovery
                    .list_databases()
                    .await
                    .unwrap()
                    .contains(&"yuzora_p8".into()));
                discovery.close().unwrap();
                runtime
                    .open_postgres(
                        "workbench",
                        "127.0.0.1",
                        55432,
                        "yuzora_p8",
                        "yuzora_full",
                        password.clone(),
                        true,
                        true,
                    )
                    .await
                    .unwrap()
            }
            "mssql" => {
                let discovery = runtime
                    .open_mssql(
                        "discovery",
                        "127.0.0.1",
                        51433,
                        "",
                        "yuzora_full",
                        password.clone(),
                        true,
                    )
                    .await
                    .unwrap();
                assert!(discovery
                    .list_databases()
                    .await
                    .unwrap()
                    .contains(&"yuzora_p8".into()));
                discovery.close().unwrap();
                runtime
                    .open_mssql(
                        "workbench",
                        "127.0.0.1",
                        51433,
                        "yuzora_p8",
                        "sa",
                        password.clone(),
                        true,
                    )
                    .await
                    .unwrap()
            }
            unknown => panic!("unknown test engine: {unknown}"),
        };
        let table = format!("v0016_edit_{}", uuid::Uuid::new_v4().simple());
        let schema = if engine == "mssql" {
            "dbo"
        } else if engine == "postgres" {
            "public"
        } else {
            "main"
        };
        let qualified = format!("{schema}.{table}");
        // Case-insensitive labels, so only the exact original-value guard built by
        // src/lib/databaseEditing.ts rejects a stale edit.
        let label_collation = match engine {
            "mssql" => "COLLATE Latin1_General_CI_AS",
            "postgres" => {
                execute(&connection, "CREATE COLLATION IF NOT EXISTS yuzora_ci (provider = icu, locale = 'und-u-ks-level2', deterministic = false)".into()).await;
                "COLLATE yuzora_ci"
            }
            _ => "COLLATE NOCASE",
        };
        execute(
            &connection,
            format!("CREATE TABLE {qualified} (id BIGINT PRIMARY KEY, label VARCHAR(100) {label_collation} NULL)"),
        )
        .await;
        execute(
            &connection,
            format!("INSERT INTO {qualified} VALUES (9007199254740993, 'O''Brien')"),
        )
        .await;
        let object = connection
            .list_tables()
            .await
            .unwrap()
            .into_iter()
            .find(|item| item.name == table)
            .unwrap();
        assert!(connection
            .table_columns(object)
            .await
            .unwrap()
            .iter()
            .any(|column| column.name == "id" && column.pk));
        let update = |original: &str| {
            let guard = match engine {
                "mssql" => format!("CAST(CAST(label AS nvarchar(max)) AS varbinary(max)) = CAST(N'{original}' AS varbinary(max))"),
                "postgres" => format!("label = E'{original}' COLLATE \"C\""),
                _ => format!("label = '{original}' COLLATE BINARY"),
            };
            format!(
                "UPDATE {qualified} SET label = 'updated' WHERE id = 9007199254740993 AND {guard}"
            )
        };
        // A concurrent case-only or trailing-space change must not satisfy the guard.
        for stale in ["o''brien", "O''Brien "] {
            assert_eq!(
                execute(&connection, update(stale)).await.as_deref(),
                Some("0")
            );
        }
        assert_eq!(
            execute(&connection, update("O''Brien")).await.as_deref(),
            Some("1")
        );
        assert_eq!(
            execute(&connection, update("O''Brien")).await.as_deref(),
            Some("0")
        );
        execute(
            &connection,
            format!("ALTER TABLE {qualified} ADD description VARCHAR(100) NULL"),
        )
        .await;
        let rename = if engine == "mssql" {
            format!(
                "EXEC [yuzora_p8].sys.sp_rename N'[dbo].[{table}].[label]', N'title', N'COLUMN'"
            )
        } else {
            format!("ALTER TABLE {qualified} RENAME COLUMN label TO title")
        };
        execute(&connection, rename).await;
        let run = connection
            .run_primary(
                "verify-edit",
                format!("SELECT id, title, description FROM {qualified}"),
            )
            .await
            .unwrap();
        let StatementExecutionResult::Rows {
            result_session: Some(result),
            ..
        } = &run.statements[0].result
        else {
            panic!("expected rows")
        };
        assert_eq!(
            result.initial_page.rows,
            vec![vec![
                DbValue::Integer {
                    value: "9007199254740993".into()
                },
                DbValue::Text {
                    value: "updated".into()
                },
                DbValue::Null
            ]]
        );
        execute(&connection, format!("DROP TABLE {qualified}")).await;
        connection.close().unwrap();
        eprintln!("{engine}: database discovery, metadata, cell update, stale edit and schema editing passed");
    }
}
