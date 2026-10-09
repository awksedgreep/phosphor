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

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let path = args.next().unwrap_or_else(|| "big.db".into());
    let n: usize = args.next().and_then(|s| s.parse().ok()).unwrap_or(2000);
    run(&path, n)
}

/// Fill `path` with `n` fake customers (and ~3 orders each). Returns the
/// real underlying error (e.g. the OS "cannot open" message) instead of
/// panicking with a bare label, so the caller exits non-zero with context
/// (#78).
fn run(path: &str, n: usize) -> Result<(), Box<dyn std::error::Error>> {
    let conn = rusqlite::Connection::open(path)?;
    // Fast-load PRAGMAs: this is a throwaway bulk fill, not a ledger.
    conn.execute_batch("PRAGMA journal_mode = MEMORY; PRAGMA synchronous = OFF;")?;
    conn.execute_batch(
        "DROP TABLE IF EXISTS customers; DROP TABLE IF EXISTS orders;
         CREATE TABLE customers(id INTEGER PRIMARY KEY, name TEXT NOT NULL,
                                city TEXT, company TEXT, balance REAL);
         CREATE TABLE orders(id INTEGER PRIMARY KEY, customer_id INTEGER,
                             product TEXT, qty INTEGER, amount REAL, region TEXT);
         BEGIN;",
    )?;

    let mut rng = rand::thread_rng();
    let regions = ["north", "south", "east", "west"];
    let mut n_orders: i64 = 0;
    {
        let mut cust = conn.prepare(
            "INSERT INTO customers(name, city, company, balance) VALUES (?1, ?2, ?3, ?4)",
        )?;
        let mut ord = conn.prepare(
            "INSERT INTO orders(customer_id, product, qty, amount, region) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
        )?;
        for i in 1..=n {
            let name: String = Name().fake();
            let city: String = CityName().fake();
            let company: String = CompanyName().fake();
            let balance: f64 = (rng.gen_range(0.0..5000.0f64) * 100.0).round() / 100.0;
            cust.execute(rusqlite::params![name, city, company, balance])?;
            for _ in 0..rng.gen_range(0..=5) {
                let product: String = Buzzword().fake();
                let qty: i64 = rng.gen_range(1..=12);
                let amount: f64 = (rng.gen_range(5.0..900.0f64) * 100.0).round() / 100.0;
                let region = regions[rng.gen_range(0..regions.len())];
                ord.execute(rusqlite::params![i as i64, product, qty, amount, region])?;
                n_orders += 1;
            }
        }
    }
    // Indexes AFTER the load (faster than maintaining them per row),
    // before COMMIT so readers never see an unindexed table. The orders
    // FK join/filter full-scanned without this.
    conn.execute_batch(
        "CREATE INDEX idx_orders_customer ON orders(customer_id);
         CREATE INDEX idx_orders_region ON orders(region);
         COMMIT;",
    )?;
    println!("{path}: {n} customers, {n_orders} orders — open it and hold PgDn");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::run;

    /// #78: a path that cannot be opened surfaces the real underlying
    /// error (not a bare panic label) so the caller can exit non-zero,
    /// and leaves no partial database behind.
    #[test]
    fn open_failure_returns_the_real_error() {
        let bad = std::env::temp_dir()
            .join(format!("phosphor-seed-nope-{}", std::process::id()))
            .join("definitely-missing-dir")
            .join("big.db");
        let err = run(bad.to_str().unwrap(), 1).unwrap_err().to_string();
        let lower = err.to_lowercase();
        assert!(
            lower.contains("open") || lower.contains("no such file") || lower.contains("sqlite"),
            "the real underlying error should surface, got: {err:?}"
        );
        assert!(!bad.exists(), "no partial database should be left behind");
    }
}
