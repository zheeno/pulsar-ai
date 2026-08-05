import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';
import { NgxPulseClient, NgxStock } from './ngx-pulse.client';
import { TradingCalendarService } from './trading-calendar.service';
import { logStart } from '../common/log.util';

@Injectable()
export class IngestionService {
  private readonly logger = new Logger(IngestionService.name);

  constructor(
    private readonly db: DatabaseService,
    private readonly redis: RedisService,
    private readonly ngx: NgxPulseClient,
    private readonly calendar: TradingCalendarService,
  ) {}

  async ingestStocks(options?: { force?: boolean }): Promise<number> {
    const log = logStart(this.logger, 'ingestStocks', { force: options?.force });
    const force = options?.force || process.env.FORCE_INGEST === 'true';
    if (!force && !this.calendar.isMarketOpen() && !this.isPostCloseWindow()) {
      log.debug('skipped', { reason: 'market closed' });
      log.done({ count: 0 });
      return 0;
    }
    try {
      const stocks = await this.ngx.getStocks();
      const tradeDate = this.calendar.todayWAT();
      let count = 0;
      for (const stock of stocks) {
        await this.upsertStock(stock, tradeDate);
        count++;
      }
      log.done({ count, tradeDate });
      return count;
    } catch (err) {
      log.warn('skipped', { error: err instanceof Error ? err.message : String(err) });
      log.done({ count: 0 });
      return 0;
    }
  }

  async ingestMarket(options?: { force?: boolean }): Promise<void> {
    const log = logStart(this.logger, 'ingestMarket', { force: options?.force });
    if (!options?.force && !this.calendar.isTradingDay()) {
      log.debug('skipped', { reason: 'not a trading day' });
      log.done();
      return;
    }
    try {
      const market = await this.ngx.getMarket();
      const tradeDate = this.calendar.todayWAT();
      if (market.asi) {
        await this.db.query(
          `INSERT INTO index_history (index_code, trade_date, value, points)
           VALUES ('ASI', $1, $2, $3)
           ON CONFLICT (index_code, trade_date) DO UPDATE SET value = EXCLUDED.value`,
          [tradeDate, market.asi.value, market.asi.change_percent || 0],
        );
      }
      log.done({ tradeDate, asi: market.asi?.value });
    } catch (err) {
      log.warn('skipped', { error: err instanceof Error ? err.message : String(err) });
      log.done();
    }
  }

  async ingestIndices(options?: { force?: boolean }): Promise<void> {
    const log = logStart(this.logger, 'ingestIndices', { force: options?.force });
    if (!options?.force && !this.calendar.isTradingDay()) {
      log.debug('skipped', { reason: 'not a trading day' });
      log.done();
      return;
    }
    try {
      const indices = await this.ngx.getIndices();
      const tradeDate = this.calendar.todayWAT();
      for (const idx of indices) {
        await this.db.query(
          `INSERT INTO index_history (index_code, trade_date, value, points, week_change, month_change, year_change)
           VALUES ($1, $2, $3, $4, $5, $6, $7)
           ON CONFLICT (index_code, trade_date) DO UPDATE SET value = EXCLUDED.value`,
          [idx.code, tradeDate, idx.value, idx.points, idx.week_change, idx.month_change, idx.year_change],
        );
      }
      log.done({ count: indices.length, tradeDate });
    } catch (err) {
      log.warn('skipped', { error: err instanceof Error ? err.message : String(err) });
      log.done();
    }
  }

  async backfillSymbols(limit = 5): Promise<number> {
    const log = logStart(this.logger, 'backfillSymbols', { limit });
    if (this.calendar.isTradingDay()) {
      log.debug('skipped', { reason: 'trading day' });
      log.done({ count: 0 });
      return 0;
    }
    const result = await this.db.query(
      `SELECT i.symbol FROM instruments i
       LEFT JOIN backfill_state b ON i.symbol = b.symbol
       WHERE i.is_active = true
       ORDER BY b.last_run_at NULLS FIRST
       LIMIT $1`,
      [limit],
    );
    let count = 0;
    for (const row of result.rows) {
      const symbol = row.symbol as string;
      const from = new Date();
      from.setFullYear(from.getFullYear() - 1);
      const history = await this.ngx.getSymbolPrice(
        symbol,
        from.toISOString().split('T')[0],
        new Date().toISOString().split('T')[0],
      );
      for (const h of history) {
        await this.db.query(
          `INSERT INTO price_history (symbol, trade_date, price, volume)
           VALUES ($1, $2, $3, $4)
           ON CONFLICT (symbol, trade_date) DO UPDATE SET price = EXCLUDED.price`,
          [symbol, h.date, h.price, h.volume || 0],
        );
      }
      await this.db.query(
        `INSERT INTO backfill_state (symbol, earliest_date_fetched, last_run_at)
         VALUES ($1, $2, now())
         ON CONFLICT (symbol) DO UPDATE SET last_run_at = now()`,
        [symbol, history[0]?.date],
      );
      count++;
    }
    log.done({ count });
    return count;
  }

  private async upsertStock(stock: NgxStock, tradeDate: string): Promise<void> {
    await this.db.query(
      `INSERT INTO instruments (symbol, name, sector, is_active)
       VALUES ($1, $2, $3, true)
       ON CONFLICT (symbol) DO UPDATE SET name = COALESCE(EXCLUDED.name, instruments.name)`,
      [stock.symbol, stock.name || stock.symbol, stock.sector || 'Unknown'],
    );
    await this.db.query(
      `INSERT INTO price_history (symbol, trade_date, price, change_percent, volume, market_cap, pe_ratio, ingested_at)
       VALUES ($1, $2, $3, $4, $5, $6, $7, now())
       ON CONFLICT (symbol, trade_date) DO UPDATE SET
         price = EXCLUDED.price, change_percent = EXCLUDED.change_percent,
         volume = EXCLUDED.volume, ingested_at = now()`,
      [stock.symbol, tradeDate, stock.price, stock.change_percent || 0, stock.volume || 0, stock.market_cap, stock.pe_ratio],
    );
    await this.redis.set(`price:${stock.symbol}`, JSON.stringify({
      symbol: stock.symbol,
      price: stock.price,
      trade_date: tradeDate,
      updated_at: new Date().toISOString(),
    }), 2400);
  }

  private isPostCloseWindow(): boolean {
    const wat = this.calendar.toWAT(new Date());
    const minutes = wat.getHours() * 60 + wat.getMinutes();
    return this.calendar.isTradingDay() && minutes >= 16 * 60 && minutes < 16 * 60 + 30;
  }
}
