#!/usr/bin/env node
const { Client } = require('pg');
const crypto = require('crypto');

const CURATED_SYMBOLS = [
  { symbol: 'DANGCEM', name: 'Dangote Cement', sector: 'Industrial Goods' },
  { symbol: 'GTCO', name: 'GTCO Plc', sector: 'Financial Services' },
  { symbol: 'ZENITHBANK', name: 'Zenith Bank', sector: 'Financial Services' },
  { symbol: 'MTNN', name: 'MTN Nigeria', sector: 'ICT' },
  { symbol: 'BUACEMENT', name: 'BUA Cement', sector: 'Industrial Goods' },
  { symbol: 'ACCESSCORP', name: 'Access Holdings', sector: 'Financial Services' },
  { symbol: 'UBA', name: 'UBA', sector: 'Financial Services' },
  { symbol: 'FBNH', name: 'FBN Holdings', sector: 'Financial Services' },
  { symbol: 'SEPLAT', name: 'Seplat Energy', sector: 'Oil & Gas' },
  { symbol: 'NESTLE', name: 'Nestle Nigeria', sector: 'Consumer Goods' },
  { symbol: 'BUAFOODS', name: 'BUA Foods', sector: 'Consumer Goods' },
  { symbol: 'AIRTELAFRI', name: 'Airtel Africa', sector: 'ICT' },
  { symbol: 'WAPCO', name: 'Lafarge Africa', sector: 'Industrial Goods' },
  { symbol: 'GUARANTY', name: 'Guaranty Trust Holding', sector: 'Financial Services' },
  { symbol: 'STANBIC', name: 'Stanbic IBTC', sector: 'Financial Services' },
  { symbol: 'FLOURMILL', name: 'Flour Mills', sector: 'Consumer Goods' },
  { symbol: 'PRESCO', name: 'Presco', sector: 'Agriculture' },
  { symbol: 'OKOMUOIL', name: 'Okomu Oil Palm', sector: 'Agriculture' },
  { symbol: 'NASCON', name: 'Nascon Allied', sector: 'Consumer Goods' },
  { symbol: 'INTBREW', name: 'International Breweries', sector: 'Consumer Goods' },
];

function hashPassword(password) {
  return crypto.createHash('sha256').update(password).digest('hex');
}

function generatePriceHistory(symbol, basePrice, days = 120) {
  const rows = [];
  const today = new Date();
  let price = basePrice;
  for (let i = days; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    if (d.getDay() === 0 || d.getDay() === 6) continue;
    const change = (Math.random() - 0.48) * 0.03;
    price = Math.max(price * (1 + change), 1);
    rows.push({
      symbol,
      trade_date: d.toISOString().split('T')[0],
      price: Math.round(price * 100) / 100,
      change_percent: Math.round(change * 10000) / 100,
      volume: Math.floor(Math.random() * 5000000) + 100000,
    });
  }
  return rows;
}

const BASE_PRICES = {
  DANGCEM: 280, GTCO: 45, ZENITHBANK: 38, MTNN: 220, BUACEMENT: 95,
  ACCESSCORP: 22, UBA: 28, FBNH: 18, SEPLAT: 3200, NESTLE: 1200,
  BUAFOODS: 150, AIRTELAFRI: 2100, WAPCO: 35, GUARANTY: 55, STANBIC: 65,
  FLOURMILL: 42, PRESCO: 280, OKOMUOIL: 350, NASCON: 18, INTBREW: 5,
};

async function seed() {
  const databaseUrl = process.env.DATABASE_URL || 'postgresql://postgres:postgres@localhost:5432/ngx_trading';
  const client = new Client({ connectionString: databaseUrl });
  await client.connect();

  for (const inst of CURATED_SYMBOLS) {
    await client.query(
      `INSERT INTO instruments (symbol, name, sector, is_active) VALUES ($1, $2, $3, true)
       ON CONFLICT (symbol) DO UPDATE SET name = EXCLUDED.name, sector = EXCLUDED.sector`,
      [inst.symbol, inst.name, inst.sector],
    );
  }

  for (const inst of CURATED_SYMBOLS) {
    const prices = generatePriceHistory(inst.symbol, BASE_PRICES[inst.symbol] || 100);
    for (const p of prices) {
      await client.query(
        `INSERT INTO price_history (symbol, trade_date, price, change_percent, volume)
         VALUES ($1, $2, $3, $4, $5)
         ON CONFLICT (symbol, trade_date) DO UPDATE SET price = EXCLUDED.price`,
        [p.symbol, p.trade_date, p.price, p.change_percent, p.volume],
      );
    }
  }

  // ASI index history
  const today = new Date();
  let asiValue = 95000;
  for (let i = 60; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    if (d.getDay() === 0 || d.getDay() === 6) continue;
    asiValue *= 1 + (Math.random() - 0.48) * 0.01;
    await client.query(
      `INSERT INTO index_history (index_code, trade_date, value, points)
       VALUES ('ASI', $1, $2, $3)
       ON CONFLICT (index_code, trade_date) DO NOTHING`,
      [d.toISOString().split('T')[0], Math.round(asiValue), Math.round((Math.random() - 0.5) * 200)],
    );
  }

  const existing = await client.query(`SELECT id FROM strategy_param_sets WHERE name = 'default-sandbox' LIMIT 1`);
  let paramId = existing.rows[0]?.id;

  if (!paramId) {
    const paramResult = await client.query(
      `INSERT INTO strategy_param_sets (name, max_position_pct, max_daily_trades, stop_loss_pct,
        min_confidence_to_trade, max_daily_drawdown_pct, allowed_symbols, position_size_pct, is_active)
       VALUES ('default-sandbox', 0.10, 5, 0.05, 0.65, 0.03, $1, 0.05, true)
       RETURNING id`,
      [CURATED_SYMBOLS.map((s) => s.symbol)],
    );
    paramId = paramResult.rows[0].id;
  }

  await client.query(`UPDATE strategy_param_sets SET is_active = false WHERE id != $1`, [paramId]);
  await client.query(`UPDATE strategy_param_sets SET is_active = true WHERE id = $1`, [paramId]);

  const startingCapital = Number(process.env.DEFAULT_STARTING_CAPITAL || 10000000);
  const existingPortfolio = await client.query(`SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1`);
  if (existingPortfolio.rows.length === 0) {
    await client.query(
      `INSERT INTO sandbox_portfolios (name, starting_capital, cash_balance, strategy_param_set_id)
       VALUES ('default-sandbox', $1, $1, $2)`,
      [startingCapital, paramId],
    );
  }

  await client.query(
    `INSERT INTO app_users (email, password_hash) VALUES ($1, $2)
     ON CONFLICT (email) DO NOTHING`,
    ['admin@ngx.local', hashPassword('admin123')],
  );

  await client.end();
  console.log('Seed complete.');
}

seed().catch((err) => {
  console.error(err);
  process.exit(1);
});
