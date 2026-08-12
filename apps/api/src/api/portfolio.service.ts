import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import {
  SandboxPortfolio,
  SandboxPortfolioDocument,
  SandboxPosition,
  SandboxPositionDocument,
  PriceHistory,
  PriceHistoryDocument,
  DailyPerformanceSnapshot,
  DailyPerformanceSnapshotDocument,
} from '../database/schemas';
import { RedisService } from '../redis/redis.service';
import { docToApi, docsToApi } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Injectable()
export class PortfolioService {
  private readonly logger = new Logger(PortfolioService.name);

  constructor(
    @InjectModel(SandboxPortfolio.name) private readonly portfolioModel: Model<SandboxPortfolioDocument>,
    @InjectModel(SandboxPosition.name) private readonly positionModel: Model<SandboxPositionDocument>,
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
    @InjectModel(DailyPerformanceSnapshot.name) private readonly snapshotModel: Model<DailyPerformanceSnapshotDocument>,
    private readonly redis: RedisService,
  ) {}

  async getPortfolio(id: string) {
    const log = logStart(this.logger, 'getPortfolio', { id });
    const portfolio = await this.portfolioModel.findById(id).exec();
    if (!portfolio) {
      log.done({ found: false });
      return null;
    }

    const positions = await this.positionModel.find({ portfolio_id: id }).exec();
    let marketValue = 0;
    const enrichedPositions = [];
    for (const pos of positions) {
      const price = await this.getPrice(pos.symbol);
      const value = Number(pos.quantity) * price;
      marketValue += value;
      enrichedPositions.push({ ...docToApi(pos), current_price: price, market_value: value });
    }

    const cashBalance = Number(portfolio.cash_balance);
    const totalEquity = cashBalance + marketValue;
    const today = new Date().toISOString().split('T')[0];
    const todaySnapshot = await this.snapshotModel.findOne({ portfolio_id: id, snapshot_date: today }).exec();

    log.done({ totalEquity, positions: enrichedPositions.length });
    return {
      portfolio: docToApi(portfolio),
      positions: enrichedPositions,
      total_equity: totalEquity,
      market_value: marketValue,
      pnl_today: Number(todaySnapshot?.pnl_daily || 0),
    };
  }

  async getPerformance(id: string) {
    const log = logStart(this.logger, 'getPerformance', { id });
    const rows = await this.snapshotModel.find({ portfolio_id: id }).sort({ snapshot_date: 1 }).exec();
    log.done({ snapshots: rows.length });
    return docsToApi(rows);
  }

  async getDefaultPortfolioId(): Promise<string | null> {
    const log = logStart(this.logger, 'getDefaultPortfolioId');
    const portfolio = await this.portfolioModel.findOne({ name: 'default-sandbox' }).exec();
    const id = portfolio?._id || null;
    log.done({ id });
    return id;
  }

  private async getPrice(symbol: string): Promise<number> {
    const cached = await this.redis.get(`price:${symbol}`);
    if (cached) return JSON.parse(cached).price;
    const row = await this.priceModel.findOne({ symbol }).sort({ trade_date: -1 }).exec();
    return Number(row?.price || 0);
  }
}
