use super::*;

fn rows(conn: &Connection, sql: &str, columns: usize) -> Vec<Vec<Value>> {
    conn.prepare(sql)
        .unwrap()
        .query_map([], |row| {
            (0..columns).map(|i| row.get::<_, Value>(i)).collect()
        })
        .unwrap()
        .collect::<crate::db::Result<_>>()
        .unwrap()
}

#[test]
fn migration_118_preserves_prices_usage_and_constraints_while_adding_ultrafast() {
    let conn = memory_connection();
    install_schema_through(&conn, 117);
    conn.pragma_update(None, "foreign_keys", "ON").unwrap();
    for (source, key, effective_from) in [
        ("derived", "derived-model", 0),
        ("admin", "admin-model", 100),
    ] {
        conn.execute(
            "INSERT INTO model_price_catalog (price_key,effective_from,display_name,source,source_note,updated_at)
             VALUES (?1,?2,?1,?3,'preserve me',123)",
            params![key, effective_from, source],
        )
        .unwrap();
        for tier in ["standard", "priority", "flex"] {
            for context in ["base", "long"] {
                conn.execute(
                    "INSERT INTO model_price_rates VALUES(?1,?2,?3,?4,1000001,2000003,11,20,?5)",
                    params![
                        key,
                        effective_from,
                        tier,
                        context,
                        (context == "long").then_some(31)
                    ],
                )
                .unwrap();
            }
        }
    }
    conn.execute_batch(
        "INSERT INTO model_price_aliases VALUES('alias','exact','admin-model',7,'admin',123);
         INSERT INTO share_listing_usage_rollup VALUES('share',86400,'admin-model','standard','base',1,2,3,4,5,6,7,123);
         INSERT INTO share_listing_usage_rollup VALUES('share',86400,'admin-model','priority','long',8,9,10,11,12,13,14,123);
         INSERT INTO share_listing_usage_rollup VALUES('share',86400,'derived-model','flex','base',15,16,17,18,19,20,21,123);",
    )
    .unwrap();
    let tables = [
        ("SELECT * FROM model_price_catalog ORDER BY 1,2", 11),
        ("SELECT * FROM model_price_rates ORDER BY 1,2,3,4", 9),
        ("SELECT * FROM model_price_aliases ORDER BY 1,2", 6),
        (
            "SELECT * FROM share_listing_usage_rollup ORDER BY 1,2,3,4,5",
            13,
        ),
        (
            "SELECT * FROM schema_migrations WHERE version <= 117 ORDER BY version",
            3,
        ),
    ];
    let before: Vec<_> = tables.iter().map(|(sql, n)| rows(&conn, sql, *n)).collect();
    apply(&conn).unwrap();
    apply(&conn).unwrap();
    check_compatibility(&conn).unwrap();
    for ((sql, columns), expected) in tables.iter().zip(before) {
        assert_eq!(rows(&conn, sql, *columns), expected, "{sql}");
    }
    for context in ["base", "long"] {
        conn.execute(
            "INSERT INTO model_price_rates VALUES('admin-model',100,'ultrafast',?1,9000001,18000003,11,20,31)",
            params![context],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO share_listing_usage_rollup VALUES('share',86400,'admin-model','ultrafast',?1,1,2,3,4,5,6,7,123)",
            params![context],
        )
        .unwrap();
    }
    // Rebuilding must keep all previous integrity guards, not just widen tiers.
    for sql in [
        "INSERT INTO model_price_rates VALUES('admin-model',100,'unknown','base',1,2,3,4,5)",
        "INSERT INTO model_price_rates VALUES('admin-model',100,'ultrafast','invalid',1,2,3,4,5)",
        "UPDATE model_price_rates SET input_micros_per_1m=-1",
        "UPDATE model_price_rates SET output_micros_per_1m=-1",
        "UPDATE model_price_rates SET cache_read_micros_per_1m=-1",
        "UPDATE model_price_rates SET cache_write_5m_micros_per_1m=-1",
        "UPDATE model_price_rates SET cache_write_1h_micros_per_1m=0",
        "INSERT INTO model_price_rates SELECT * FROM model_price_rates LIMIT 1",
        "INSERT INTO model_price_rates VALUES('missing-model',0,'ultrafast','base',1,2,3,4,5)",
        "UPDATE share_listing_usage_rollup SET service_tier='unknown'",
        "UPDATE share_listing_usage_rollup SET context_tier='invalid'",
        "UPDATE share_listing_usage_rollup SET bucket_start=1",
        "UPDATE share_listing_usage_rollup SET bucket_start=-86400",
        "UPDATE share_listing_usage_rollup SET input_tokens=-1",
        "UPDATE share_listing_usage_rollup SET output_tokens=-1",
        "UPDATE share_listing_usage_rollup SET cache_read_tokens=-1",
        "UPDATE share_listing_usage_rollup SET cache_write_tokens=-1",
        "UPDATE share_listing_usage_rollup SET unattributed_tokens=-1",
        "UPDATE share_listing_usage_rollup SET distinct_users=-1",
        "UPDATE share_listing_usage_rollup SET request_count=-1",
        "INSERT INTO share_listing_usage_rollup SELECT * FROM share_listing_usage_rollup LIMIT 1",
    ] {
        assert!(conn.execute(sql, []).is_err(), "must reject: {sql}");
    }
    assert_eq!(
        conn.query_row("SELECT count(*) FROM pragma_foreign_key_check", [], |r| r
            .get::<_, i64>(
            0
        ))
        .unwrap(),
        0
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM pragma_index_info('idx_share_listing_usage_rollup_window')",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    conn.execute(
        "DELETE FROM model_price_catalog WHERE price_key='admin-model'",
        [],
    )
    .unwrap();
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM model_price_rates WHERE price_key='admin-model'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0,
        "catalog deletion must still cascade to all service tiers"
    );
    assert_eq!(
        conn.query_row(
            "SELECT count(*) FROM model_price_rates WHERE price_key='derived-model'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        6
    );
}
