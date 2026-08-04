//! phosphor-seed: fill a database with realistic fake people so record
//! paging (PgDn in EDIT) has something worth flying through.
//!
//!     cargo run --release --features seed --bin phosphor-seed -- \
//!         big.db [customers=2000]
//!
//! Uses the `fake` crate for names/cities/companies; orders get ~3 rows
//! per customer. Rerunnable: drops and recreates both tables.

use fake::faker::address::en::CityName;
use fake::faker::company::en::{Buzzword, CompanyName};
use fake::faker::name::en::Name;
use fake::Fake;
use rand::Rng;

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "big.db".into());
    let n: usize = args
        .next()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2000);

    let conn = rusqlite::Connection::open(&path).expect("open db");
    conn.execute_batch(
        "DROP TABLE IF EXISTS customers; DROP TABLE IF EXISTS orders;
         CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                                city TEXT, company TEXT, balance REAL);
         CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER,
                             product TEXT, qty INTEGER, amount REAL, region TEXT);
         BEGIN;",
    )
    .expect("schema");

    let mut rng = rand::thread_rng();
    let regions = ["north", "south", "east", "west"];
    {
        let mut cust = conn
            .prepare("INSERT INTO customers(name, city, company, balance) VALUES (?1, ?2, ?3, ?4)")
            .unwrap();
        let mut ord = conn
            .prepare(
                "INSERT INTO orders(customer_id, product, qty, amount, region) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
            )
            .unwrap();
        for i in 1..=n {
            let name: String = Name().fake();
            let city: String = CityName().fake();
            let company: String = CompanyName().fake();
            let balance: f64 = (rng.gen_range(0.0..5000.0f64) * 100.0).round() / 100.0;
            cust.execute(rusqlite::params![name, city, company, balance])
                .unwrap();
            for _ in 0..rng.gen_range(0..=5) {
                let product: String = Buzzword().fake();
                let qty: i64 = rng.gen_range(1..=12);
                let amount: f64 = (rng.gen_range(5.0..900.0f64) * 100.0).round() / 100.0;
                let region = regions[rng.gen_range(0..regions.len())];
                ord.execute(rusqlite::params![i as i64, product, qty, amount, region])
                    .unwrap();
            }
        }
    }
    conn.execute_batch("COMMIT;").unwrap();
    let orders: i64 = conn
        .query_row("SELECT count(*) FROM orders", [], |r| r.get(0))
        .unwrap();
    println!("{path}: {n} customers, {orders} orders — open it and hold PgDn");
}
