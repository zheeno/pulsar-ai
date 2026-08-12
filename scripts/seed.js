#!/usr/bin/env node
const { MongoClient } = require('mongodb');
const bcrypt = require('bcrypt');
const { randomUUID } = require('crypto');

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

const BASE_PRICES = {
  DANGCEM: 280, GTCO: 45, ZENITHBANK: 38, MTNN: 220, BUACEMENT: 95,
  ACCESSCORP: 22, UBA: 28, FBNH: 18, SEPLAT: 3200, NESTLE: 1200,
  BUAFOODS: 150, AIRTELAFRI: 2100, WAPCO: 35, GUARANTY: 55, STANBIC: 65,
  FLOURMILL: 42, PRESCO: 280, OKOMUOIL: 350, NASCON: 18, INTBREW: 5,
};

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
      _id: randomUUID(),
      symbol,
      trade_date: d.toISOString().split('T')[0],
      price: Math.round(price * 100) / 100,
      change_percent: Math.round(change * 10000) / 100,
      volume: Math.floor(Math.random() * 5000000) + 100000,
    });
  }
  return rows;
}

async function seed() {
  const uri = process.env.MONGODB_URI || 'mongodb://localhost:27017/pulsar';
  const client = new MongoClient(uri);
  await client.connect();
  const db = client.db();

  for (const inst of CURATED_SYMBOLS) {
    await db.collection('instruments').updateOne(
      { symbol: inst.symbol },
      {
        $setOnInsert: { _id: randomUUID(), added_at: new Date() },
        $set: { symbol: inst.symbol, name: inst.name, sector: inst.sector, is_active: true },
      },
      { upsert: true },
    );
  }

  for (const inst of CURATED_SYMBOLS) {
    const prices = generatePriceHistory(inst.symbol, BASE_PRICES[inst.symbol] || 100);
    for (const p of prices) {
      await db.collection('price_history').updateOne(
        { symbol: p.symbol, trade_date: p.trade_date },
        { $set: p },
        { upsert: true },
      );
    }
  }

  const today = new Date();
  let asiValue = 95000;
  for (let i = 60; i >= 0; i--) {
    const d = new Date(today);
    d.setDate(d.getDate() - i);
    if (d.getDay() === 0 || d.getDay() === 6) continue;
    asiValue *= 1 + (Math.random() - 0.48) * 0.01;
    const tradeDate = d.toISOString().split('T')[0];
    await db.collection('index_history').updateOne(
      { index_code: 'ASI', trade_date: tradeDate },
      {
        $setOnInsert: { _id: randomUUID() },
        $set: {
          index_code: 'ASI',
          trade_date: tradeDate,
          value: Math.round(asiValue),
          points: Math.round((Math.random() - 0.5) * 200),
        },
      },
      { upsert: true },
    );
  }

  let param = await db.collection('strategy_param_sets').findOne({ name: 'default-sandbox' });
  if (!param) {
    param = {
      _id: randomUUID(),
      name: 'default-sandbox',
      max_position_pct: 0.1,
      max_daily_trades: 5,
      stop_loss_pct: 0.05,
      min_confidence_to_trade: 0.65,
      max_daily_drawdown_pct: 0.03,
      allowed_symbols: null,
      position_size_pct: 0.05,
      is_active: true,
      created_at: new Date(),
    };
    await db.collection('strategy_param_sets').insertOne(param);
  } else {
    await db.collection('strategy_param_sets').updateOne(
      { _id: param._id },
      { $set: { allowed_symbols: null, is_active: true } },
    );
  }

  await db.collection('strategy_param_sets').updateMany(
    { _id: { $ne: param._id } },
    { $set: { is_active: false } },
  );

  const startingCapital = Number(process.env.DEFAULT_STARTING_CAPITAL || 10000000);
  await db.collection('sandbox_portfolios').updateOne(
    { name: 'default-sandbox' },
    {
      $setOnInsert: { _id: randomUUID(), created_at: new Date() },
      $set: {
        name: 'default-sandbox',
        starting_capital: startingCapital,
        cash_balance: startingCapital,
        strategy_param_set_id: param._id,
      },
    },
    { upsert: true },
  );

  const passwordHash = await bcrypt.hash('admin123', 10);
  await db.collection('users').updateOne(
    { email: 'admin@ngx.local' },
    {
      $setOnInsert: { _id: randomUUID(), created_at: new Date() },
      $set: { email: 'admin@ngx.local', password_hash: passwordHash },
    },
    { upsert: true },
  );

  await client.close();
  console.log('MongoDB seed complete.');
}

seed().catch((err) => {
  console.error(err);
  process.exit(1);
});
