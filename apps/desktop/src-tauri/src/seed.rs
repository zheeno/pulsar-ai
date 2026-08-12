use anyhow::Result;
use rusqlite::Connection;
use uuid::Uuid;

pub struct SeedService;

const CURATED: &[(&str, &str, &str)] = &[
    ("DANGCEM", "Dangote Cement", "Industrial Goods"),
    ("GTCO", "GTCO Plc", "Financial Services"),
    ("ZENITHBANK", "Zenith Bank", "Financial Services"),
    ("MTNN", "MTN Nigeria", "ICT"),
    ("BUACEMENT", "BUA Cement", "Industrial Goods"),
    ("ACCESSCORP", "Access Holdings", "Financial Services"),
    ("UBA", "UBA", "Financial Services"),
    ("FBNH", "FBN Holdings", "Financial Services"),
    ("SEPLAT", "Seplat Energy", "Oil & Gas"),
    ("NESTLE", "Nestle Nigeria", "Consumer Goods"),
    ("BUAFOODS", "BUA Foods", "Consumer Goods"),
    ("AIRTELAFRI", "Airtel Africa", "ICT"),
    ("WAPCO", "Lafarge Africa", "Industrial Goods"),
    ("GUARANTY", "Guaranty Trust Holding", "Financial Services"),
    ("STANBIC", "Stanbic IBTC", "Financial Services"),
    ("FLOURMILL", "Flour Mills", "Consumer Goods"),
    ("PRESCO", "Presco", "Agriculture"),
    ("OKOMUOIL", "Okomu Oil Palm", "Agriculture"),
    ("NASCON", "Nascon Allied", "Consumer Goods"),
    ("INTBREW", "International Breweries", "Consumer Goods"),
];

const BASE_PRICES: &[(&str, f64)] = &[
    ("DANGCEM", 280.0), ("GTCO", 45.0), ("ZENITHBANK", 38.0), ("MTNN", 220.0), ("BUACEMENT", 95.0),
    ("ACCESSCORP", 22.0), ("UBA", 28.0), ("FBNH", 18.0), ("SEPLAT", 3200.0), ("NESTLE", 1200.0),
    ("BUAFOODS", 150.0), ("AIRTELAFRI", 2100.0), ("WAPCO", 35.0), ("GUARANTY", 55.0), ("STANBIC", 65.0),
    ("FLOURMILL", 42.0), ("PRESCO", 280.0), ("OKOMUOIL", 350.0), ("NASCON", 18.0), ("INTBREW", 5.0),
];

impl SeedService {
    pub fn seed_if_empty(conn: &Connection, starting_capital: f64) -> Result<()> {
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM instruments", [], |row| row.get(0))?;
        if count > 0 {
            return Ok(());
        }
        Self::seed_all(conn, starting_capital)
    }

    pub fn seed_all(conn: &Connection, starting_capital: f64) -> Result<()> {
        for (symbol, name, sector) in CURATED {
            conn.execute(
                "INSERT OR IGNORE INTO instruments (symbol, name, sector, is_active) VALUES (?1, ?2, ?3, 1)",
                rusqlite::params![symbol, name, sector],
            )?;
        }

        for (symbol, name, sector) in CURATED {
            let base = BASE_PRICES.iter().find(|(s, _)| *s == *symbol).map(|(_, p)| *p).unwrap_or(100.0);
            let prices = generate_price_history(symbol, base, 120);
            for p in prices {
                conn.execute(
                    "INSERT OR IGNORE INTO price_history (symbol, trade_date, price, change_percent, volume)
                     VALUES (?1, ?2, ?3, ?4, ?5)",
                    rusqlite::params![p.0, p.1, p.2, p.3, p.4],
                )?;
            }
            let _ = (name, sector);
        }

        let param_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO strategy_param_sets (id, name, is_active, allowed_symbols)
             VALUES (?1, 'default', 1, ?2)",
            rusqlite::params![
                param_id,
                serde_json::to_string(&CURATED.iter().map(|(s, _, _)| *s).collect::<Vec<_>>())?
            ],
        )?;

        let portfolio_id = Uuid::new_v4().to_string();
        conn.execute(
            "INSERT INTO sandbox_portfolios (id, name, starting_capital, cash_balance, strategy_param_set_id)
             VALUES (?1, 'default-sandbox', ?2, ?2, ?3)",
            rusqlite::params![portfolio_id, starting_capital, param_id],
        )?;

        Ok(())
    }
}

fn generate_price_history(symbol: &str, base_price: f64, days: i64) -> Vec<(String, String, f64, f64, i64)> {
    let _ = symbol;
    let mut rows = vec![];
    let today = chrono::Utc::now();
    let mut price = base_price;
    for i in (0..=days).rev() {
        let d = today - chrono::Duration::days(i);
        if d.weekday().num_days_from_monday() >= 5 {
            continue;
        }
        let change = (rand_simple(i) - 0.48) * 0.03;
        price = (price * (1.0 + change)).max(1.0);
        rows.push((
            symbol.to_string(),
            d.format("%Y-%m-%d").to_string(),
            (price * 100.0).round() / 100.0,
            (change * 10000.0).round() / 100.0,
            100_000 + (i * 1000) as i64,
        ));
    }
    rows
}

fn rand_simple(seed: i64) -> f64 {
    let x = (seed.wrapping_mul(1103515245).wrapping_add(12345)) & 0x7fffffff;
    (x as f64) / 0x7fffffff as f64
}

use chrono::Datelike;
