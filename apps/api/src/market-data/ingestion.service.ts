import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import {
  Instrument,
  InstrumentDocument,
  PriceHistory,
  PriceHistoryDocument,
  IndexHistory,
  IndexHistoryDocument,
  BackfillState,
  BackfillStateDocument,
} from '../database/schemas';
import { RedisService } from '../redis/redis.service';
import { NgxPulseClient, NgxStock } from './ngx-pulse.client';
import { TradingCalendarService } from './trading-calendar.service';
import { logStart } from '../common/log.util';

@Injectable()
export class IngestionService {
  private readonly logger = new Logger(IngestionService.name);

  constructor(
    @InjectModel(Instrument.name) private readonly instrumentModel: Model<InstrumentDocument>,
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
    @InjectModel(IndexHistory.name) private readonly indexModel: Model<IndexHistoryDocument>,
    @InjectModel(BackfillState.name) private readonly backfillModel: Model<BackfillStateDocument>,
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
        await this.indexModel.findOneAndUpdate(
          { index_code: 'ASI', trade_date: tradeDate },
          { value: market.asi.value, points: market.asi.change_percent || 0 },
          { upsert: true, new: true },
        ).exec();
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
        await this.indexModel.findOneAndUpdate(
          { index_code: idx.code, trade_date: tradeDate },
          {
            value: idx.value,
            points: idx.points,
            week_change: idx.week_change,
            month_change: idx.month_change,
            year_change: idx.year_change,
          },
          { upsert: true, new: true },
        ).exec();
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

    const instruments = await this.instrumentModel.find({ is_active: true }).exec();
    const backfillStates = await this.backfillModel.find().exec();
    const stateMap = new Map(backfillStates.map((s) => [s.symbol, s.last_run_at]));
    const symbols = instruments
      .map((i) => i.symbol)
      .sort((a, b) => {
        const aTime = stateMap.get(a)?.getTime() ?? 0;
        const bTime = stateMap.get(b)?.getTime() ?? 0;
        return aTime - bTime;
      })
      .slice(0, limit);

    let count = 0;
    for (const symbol of symbols) {
      const from = new Date();
      from.setFullYear(from.getFullYear() - 1);
      const history = await this.ngx.getSymbolPrice(
        symbol,
        from.toISOString().split('T')[0],
        new Date().toISOString().split('T')[0],
      );
      for (const h of history) {
        await this.priceModel.findOneAndUpdate(
          { symbol, trade_date: h.date },
          { price: h.price, volume: h.volume || 0 },
          { upsert: true, new: true },
        ).exec();
      }
      await this.backfillModel.findOneAndUpdate(
        { symbol },
        { earliest_date_fetched: history[0]?.date, last_run_at: new Date() },
        { upsert: true, new: true },
      ).exec();
      count++;
    }
    log.done({ count });
    return count;
  }

  private async upsertStock(stock: NgxStock, tradeDate: string): Promise<void> {
    await this.instrumentModel.findOneAndUpdate(
      { symbol: stock.symbol },
      {
        symbol: stock.symbol,
        name: stock.name || stock.symbol,
        sector: stock.sector || 'Unknown',
        is_active: true,
      },
      { upsert: true, new: true },
    ).exec();

    await this.priceModel.findOneAndUpdate(
      { symbol: stock.symbol, trade_date: tradeDate },
      {
        price: stock.price,
        change_percent: stock.change_percent || 0,
        volume: stock.volume || 0,
        market_cap: stock.market_cap,
        pe_ratio: stock.pe_ratio,
        ingested_at: new Date(),
      },
      { upsert: true, new: true },
    ).exec();

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
