use crate::*;

#[test]
fn split_and_caret() {
    let sql = "select 1;\n-- comment; not a split\nselect 'a;b', \"x;y\" from t; /* ; */\n\ncreate function f() returns int as $$ begin return 1; end $$ language plpgsql;\nselect $1;";
    let parts: Vec<&str> = split_statements(sql, Driver::Postgres).into_iter().map(|r| &sql[r]).collect();
    assert_eq!(parts.len(), 4, "{parts:?}");
    assert_eq!(parts[0], "select 1");
    assert!(parts[1].starts_with("-- comment") && parts[1].ends_with("from t"));
    assert!(parts[2].contains("return 1; end $$"));
    assert_eq!(parts[3], "select $1");
    // Caret right after `;` belongs to the statement before it.
    let r = statement_at(sql, 9, Driver::Postgres).unwrap();
    assert_eq!(&sql[r], "select 1");
    let r = statement_at(sql, sql.find("from t").unwrap(), Driver::Postgres).unwrap();
    assert!(sql[r].ends_with("from t"));
    assert!(split_statements("  -- only a comment\n /* x */ ", Driver::Sqlite).is_empty());
}

#[test]
fn backslashes_in_strings_per_dialect() {
    let parts = |sql: &str, d: Driver| -> Vec<String> { split_statements(sql, d).into_iter().map(|r| sql[r].to_string()).collect() };
    // Standard strings (PostgreSQL, SQLite): a backslash is just a character.
    let sql = r"select 'C:\'; select 2";
    assert_eq!(parts(sql, Driver::Postgres), [r"select 'C:\'", "select 2"]);
    assert_eq!(parts(sql, Driver::Sqlite), [r"select 'C:\'", "select 2"]);
    // E'…' in PostgreSQL and every MySQL string: it escapes the quote.
    let sql = r"select E'it\'s; fine'; select 2";
    assert_eq!(parts(sql, Driver::Postgres), [r"select E'it\'s; fine'", "select 2"]);
    let sql = r#"select 'it\'s; fine', "a\"; b"; select 2"#;
    assert_eq!(parts(sql, Driver::Mysql), [r#"select 'it\'s; fine', "a\"; b""#, "select 2"]);
    // `name'…'` is not an E string.
    let sql = r"select note'x\'; select 2";
    assert_eq!(parts(sql, Driver::Postgres).len(), 2);
    // Non-ASCII text between the pieces.
    assert_eq!(parts("select 'é;' as ключ; select 2", Driver::Postgres), ["select 'é;' as ключ", "select 2"]);
}

#[test]
fn urls() {
    let mut ds = DataSource::new(Driver::Sqlite);
    let pw = ds.apply_url("jdbc:postgresql://bob:p%40ss@db.example.com:6543/shop?sslmode=require").unwrap();
    assert_eq!(pw.as_deref(), Some("p@ss"));
    assert_eq!(
        (ds.driver, ds.host.as_str(), ds.port, ds.user.as_str(), ds.database.as_str(), ds.ssl),
        (Driver::Postgres, "db.example.com", 6543, "bob", "shop", Ssl::Require)
    );
    assert_eq!(ds.display_name(), "shop@db.example.com");
    let mut m = DataSource::new(Driver::Postgres);
    assert_eq!(m.apply_url("mysql://root@localhost/app").unwrap(), None);
    assert_eq!((m.driver, m.port, m.database.as_str()), (Driver::Mysql, 3306, "app"));
    let mut s = DataSource::new(Driver::Postgres);
    s.apply_url("jdbc:sqlite:$PROJECT_DIR$/db.sqlite3").unwrap();
    assert_eq!((s.driver, s.file.as_str()), (Driver::Sqlite, "db.sqlite3"));
    assert!(DataSource::new(Driver::Mysql).apply_url("http://x").is_none());
}

#[test]
fn sqlite_end_to_end() {
    let dir = std::env::temp_dir().join(format!("brisk-db-{}", new_id()));
    std::fs::create_dir_all(&dir).unwrap();
    let mut ds = DataSource::new(Driver::Sqlite);
    ds.file = "test.db".into();
    let mut c = connect(&ds, "", &dir).unwrap();
    let script = "create table users(id integer primary key, name text not null, score real, avatar blob);
        insert into users(name, score, avatar) values ('ann', 1.5, x'CAFE'), ('bob', null, null), ('кот', 3, null);
        create view top as select name from users where score > 1;
        select * from users order by id;";
    let (outs, err) = execute(c.as_mut(), script, 2);
    assert!(err.is_none(), "{err:?}");
    assert_eq!(outs.len(), 4);
    assert_eq!(outs[1].affected, Some(3));
    let sel = &outs[3];
    assert_eq!(sel.columns, ["id", "name", "score", "avatar"]);
    assert_eq!(sel.rows.len(), 2);
    assert!(sel.truncated);
    assert_eq!(sel.rows[0], vec![Some("1".into()), Some("ann".into()), Some("1.5".into()), Some("0xCAFE".into())]);
    assert_eq!(sel.rows[1][2], None);
    assert!(sel.summary().starts_with("2+ rows retrieved"));
    let (_, err) = execute(c.as_mut(), "select * from nope", 10);
    assert!(err.unwrap().contains("no such table"));

    let schema = introspect(c.as_mut()).unwrap();
    assert_eq!(schema.len(), 1);
    let t = &schema[0].tables;
    assert_eq!(t.iter().map(|t| (t.name.as_str(), t.kind)).collect::<Vec<_>>(), [("users", TableKind::Table), ("top", TableKind::View)]);
    assert!(t[0].columns[0].primary_key && !t[0].columns[1].nullable && t[0].columns[2].nullable);
    assert_eq!(select_all(Driver::Sqlite, "main", "users"), "SELECT * FROM \"users\"");
    assert!(is_ddl("/* x */ CREATE TABLE a(b int)") && !is_ddl("select 1"));
    assert!(test_connection(&ds, "", &dir).unwrap().starts_with("SQLite 3."));
    drop(c);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn sqlite_cancel() {
    let dir = std::env::temp_dir();
    let mut ds = DataSource::new(Driver::Sqlite);
    ds.file = dir.join(format!("brisk-cancel-{}.db", new_id())).to_string_lossy().into_owned();
    let mut c = connect(&ds, "", &dir).unwrap();
    let cancel = c.canceller();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(150));
        cancel.cancel();
    });
    let t0 = std::time::Instant::now();
    let r = c.run("with recursive n(x) as (select 1 union all select x + 1 from n) select count(*) from n", 10);
    t.join().unwrap();
    assert!(r.unwrap_err().contains("interrupt"));
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    drop(c);
    let _ = std::fs::remove_file(&ds.file);
}

#[test]
fn config_and_idea_import() {
    let root = std::env::temp_dir().join(format!("brisk-cfg-{}", new_id()));
    std::fs::create_dir_all(root.join(".idea")).unwrap();
    std::fs::write(
        root.join(".idea/dataSources.xml"),
        r#"<?xml version="1.0" encoding="UTF-8"?>
<project version="4">
  <component name="DataSourceManagerImpl" format="xml" multifile-model="true">
    <data-source source="LOCAL" name="shop@localhost" uuid="aaa-1">
      <driver-ref>postgresql</driver-ref>
      <jdbc-url>jdbc:postgresql://localhost:5433/shop</jdbc-url>
    </data-source>
    <data-source source="LOCAL" name="db" uuid="bbb-2">
      <driver-ref>sqlite.xerial</driver-ref>
      <jdbc-url>jdbc:sqlite:$PROJECT_DIR$/db.sqlite3</jdbc-url>
    </data-source>
  </component>
</project>"#,
    )
    .unwrap();
    std::fs::write(
        root.join(".idea/dataSources.local.xml"),
        r#"<project version="4"><component name="dataSourceStorageLocal"><data-source name="shop@localhost" uuid="aaa-1"><user-name>shopper</user-name></data-source></component></project>"#,
    )
    .unwrap();
    let (list, imported) = load_sources(&root);
    assert!(imported);
    assert_eq!(list.len(), 2);
    assert_eq!((list[0].id.as_str(), list[0].name.as_str(), list[0].port, list[0].user.as_str()), ("aaa-1", "shop@localhost", 5433, "shopper"));
    assert_eq!((list[1].driver, list[1].file.as_str()), (Driver::Sqlite, "db.sqlite3"));
    save_sources(&root, &list).unwrap();
    let (again, imported) = load_sources(&root);
    assert!(!imported);
    assert_eq!(again, list);
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn password_sealing() {
    assert_eq!(crate::secrets::roundtrip("pä$$ wörd").as_deref(), Some("pä$$ wörd"));
}

/// Live server tests: `BRISK_TEST_PG_URL=postgresql://postgres:pw@127.0.0.1:5432/postgres cargo test -p brisk-db -- --ignored`
fn live(var: &str) -> Option<(DataSource, String)> {
    let url = std::env::var(var).ok()?;
    let mut ds = DataSource::new(Driver::Postgres);
    let pw = ds.apply_url(&url)?.unwrap_or_default();
    Some((ds, pw))
}

fn run1(c: &mut dyn Connection, sql: &str) -> QueryOutput {
    let (mut o, e) = execute(c, sql, DEFAULT_LIMIT);
    assert!(e.is_none(), "{sql}: {e:?}");
    o.pop().unwrap()
}

#[test]
#[ignore]
fn postgres_live() {
    let Some((mut ds, pw)) = live("BRISK_TEST_PG_URL") else { return eprintln!("BRISK_TEST_PG_URL not set") };
    let root = std::env::temp_dir();
    let v = test_connection(&ds, &pw, &root).unwrap();
    assert!(v.starts_with("PostgreSQL "), "{v}");
    assert!(test_connection(&ds, "wrong", &root).unwrap_err().contains("28P01"));
    let mut c = connect(&ds, &pw, &root).unwrap();
    let c = c.as_mut();
    let _ = execute(c, "drop schema if exists brisk_t cascade", 10);
    let script = "create schema brisk_t;
        create table brisk_t.items(id serial primary key, name text not null, tags text[], meta jsonb, at timestamptz default '2026-01-02 03:04:05+00', price numeric(10,2));
        insert into brisk_t.items(name, tags, meta, price) select 'item ' || g, array['a','b;c'], jsonb_build_object('n', g), g * 1.5 from generate_series(1, 1200) g;
        insert into brisk_t.items(name) values ('кот');
        create view brisk_t.cheap as select id, name from brisk_t.items where price < 10;
        do $$ begin perform 1; raise notice 'x;y'; end $$;";
    let (outs, err) = execute(c, script, 10);
    assert!(err.is_none(), "{err:?}");
    assert_eq!(outs.len(), 6);
    assert_eq!(outs[2].affected, Some(1200));
    // Big read is capped on the server; truncated flag set.
    let o = run1(c, "select * from brisk_t.items order by id");
    assert_eq!(o.rows.len(), DEFAULT_LIMIT);
    assert!(o.truncated);
    assert_eq!(o.columns, ["id", "name", "tags", "meta", "at", "price"]);
    assert_eq!(o.rows[0][2].as_deref(), Some("{a,b;c}"));
    assert_eq!(o.rows[0][3].as_deref(), Some("{\"n\": 1}"));
    assert_eq!(o.rows[0][5].as_deref(), Some("1.50"));
    let o = run1(c, "select name, price from brisk_t.items where price is null");
    assert_eq!((o.rows.len(), o.rows[0][0].as_deref(), o.rows[0][1].clone()), (1, Some("кот"), None));
    // Not wrapped (locking clause), still works.
    let o = run1(c, "select id from brisk_t.items where id = 1 for update");
    assert_eq!(o.rows, vec![vec![Some("1".to_string())]]);
    let o = run1(c, "update brisk_t.items set name = name where id <= 7");
    assert_eq!((o.affected, o.has_rows()), (Some(7), false));
    let e = execute(c, "select * from brisk_t.nope", 10).1.unwrap();
    assert!(e.starts_with("[42P01] ERROR: relation \"brisk_t.nope\" does not exist"), "{e}");
    // Introspection.
    let schemas = introspect(c).unwrap();
    let s = schemas.iter().find(|s| s.name == "brisk_t").unwrap();
    assert_eq!(s.tables.iter().map(|t| (t.name.as_str(), t.kind)).collect::<Vec<_>>(), [("items", TableKind::Table), ("cheap", TableKind::View)]);
    let cols = &s.tables[0].columns;
    assert!(cols[0].primary_key && !cols[0].nullable && cols[2].nullable);
    assert_eq!((cols[1].type_name.as_str(), cols[2].type_name.as_str(), cols[3].type_name.as_str()), ("text", "text[]", "jsonb"));
    assert!(schemas.iter().any(|s| s.name == "public"));
    // Cancel a long statement from another thread.
    let cancel = c.canceller();
    let t = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancel.cancel();
    });
    let t0 = std::time::Instant::now();
    let e = c.run("select pg_sleep(20)", 10).unwrap_err();
    t.join().unwrap();
    assert!(e.contains("57014"), "{e}");
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    // The connection is still usable after a cancel.
    assert_eq!(run1(c, "select 1 as x").rows, vec![vec![Some("1".to_string())]]);
    run1(c, "drop schema brisk_t cascade");
    // SSL modes against a server without TLS.
    ds.ssl = Ssl::Disable;
    assert!(test_connection(&ds, &pw, &root).is_ok());
    ds.ssl = Ssl::Require;
    assert!(test_connection(&ds, &pw, &root).is_err());
}

#[test]
#[ignore]
fn mysql_live() {
    let Some((ds, pw)) = live("BRISK_TEST_MYSQL_URL") else { return eprintln!("BRISK_TEST_MYSQL_URL not set") };
    let root = std::env::temp_dir();
    let v = test_connection(&ds, &pw, &root).unwrap();
    assert!(v.starts_with("MySQL ") || v.starts_with("MariaDB "), "{v}");
    let mut c = connect(&ds, &pw, &root).unwrap();
    let c = c.as_mut();
    let script = "drop table if exists brisk_items;
        create table brisk_items(id int auto_increment primary key, name varchar(50) not null, at datetime, price decimal(10,2), data blob);
        insert into brisk_items(name, at, price) values ('a', '2026-01-02 03:04:05', 1.5), ('кот', null, null);";
    let (outs, err) = execute(c, script, 10);
    assert!(err.is_none(), "{err:?}");
    assert_eq!(outs[2].affected, Some(2));
    let o = run1(c, "select * from brisk_items order by id");
    assert_eq!(o.columns, ["id", "name", "at", "price", "data"]);
    assert_eq!(o.rows[0][..4], [Some("1".into()), Some("a".into()), Some("2026-01-02 03:04:05".into()), Some("1.50".into())]);
    assert_eq!(o.rows[1][1].as_deref(), Some("кот"));
    assert_eq!(o.rows[1][2], None);
    let e = execute(c, "select * from nope", 10).1.unwrap();
    assert!(e.starts_with("[42S02]"), "{e}");
    let schemas = introspect(c).unwrap();
    let t = schemas.iter().flat_map(|s| &s.tables).find(|t| t.name == "brisk_items").unwrap();
    assert!(t.columns[0].primary_key && !t.columns[1].nullable);
    assert_eq!(t.columns[1].type_name, "varchar(50)");
    let cancel = c.canceller();
    let th = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(300));
        cancel.cancel();
    });
    let t0 = std::time::Instant::now();
    let _ = c.run("select sleep(20)", 10);
    th.join().unwrap();
    assert!(t0.elapsed() < std::time::Duration::from_secs(5));
    run1(c, "drop table brisk_items");
}
