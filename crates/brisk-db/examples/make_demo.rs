//! Creates the demo SQLite database and data source for `examples/demo_project`.
//! `cargo run -p brisk-db --example make_demo -- examples/demo_project`

use brisk_db::{connect, execute, save_sources, DataSource, Driver};
use std::path::PathBuf;

fn main() {
    let root = PathBuf::from(std::env::args().nth(1).unwrap_or_else(|| "examples/demo_project".into()));
    std::fs::create_dir_all(root.join("data")).unwrap();
    let _ = std::fs::remove_file(root.join("data/shop.db"));
    let mut ds = DataSource::new(Driver::Sqlite);
    ds.id = "demo-shop-sqlite".into();
    ds.name = "shop.db".into();
    ds.file = "data/shop.db".into();
    let mut c = connect(&ds, "", &root).unwrap();
    let mut script = String::from(
        "create table customers(id integer primary key, name text not null, email text unique, created_at text default current_timestamp);
         create table orders(id integer primary key, customer_id integer not null references customers(id), total real not null, status text not null);
         create view big_orders as select o.id, c.name, o.total from orders o join customers c on c.id = o.customer_id where o.total > 100;
         insert into customers(name, email) values ('Ann Smith', 'ann@example.com'), ('Bob Lee', 'bob@example.com'), ('Иван Петров', null);",
    );
    for i in 1..=1200 {
        let status = ["new", "paid", "shipped"][i % 3];
        script.push_str(&format!("insert into orders(customer_id, total, status) values ({}, {}.{:02}, '{status}');", i % 3 + 1, (i * 37) % 250, i % 100));
    }
    let (outs, err) = execute(c.as_mut(), &script, 10);
    assert!(err.is_none(), "{err:?}");
    println!("{} statements", outs.len());
    save_sources(&root, &[ds]).unwrap();
}
