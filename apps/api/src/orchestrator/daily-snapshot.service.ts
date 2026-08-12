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
  IndexHistory,
  IndexHistoryDocument,
} from '../database/schemas';
import { RedisService } from '../redis/redis.service';
import { logStart } from '../common/log.util';

@Injectable()
export class DailySnapshotService {
  private readonly logger = new Logger(DailySnapshotService.name);

  constructor(
    @InjectModel(SandboxPortfolio.name) private readonly portfolioModel: Model<SandboxPortfolioDocument>,
    @InjectModel(SandboxPosition.name) private readonly positionModel: Model<SandboxPositionDocument>,
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
    @InjectModel(DailyPerformanceSnapshot.name) private readonly snapshotModel: Model<DailyPerformanceSnapshotDocument>,
    @InjectModel(IndexHistory.name) private readonly indexModel: Model<IndexHistoryDocument>,
    private readonly redis: RedisService,
  ) {}

  async createSnapshot(portfolioId?: string): Promise<void> {
    const log = logStart(this.logger, 'createSnapshot', { portfolioId });
    const portfolios = portfolioId
      ? await this.portfolioModel.find({ _id: portfolioId }).exec()
      : await this.portfolioModel.find().exec();

    for (const portfolio of portfolios) {
      await this.snapshotPortfolio(portfolio);
    }
    log.done({ portfolios: portfolios.length });
  }

  private async snapshotPortfolio(portfolio: SandboxPortfolioDocument): Promise<void> {
    const log = logStart(this.logger, 'snapshotPortfolio', { portfolioId: portfolio._id });
    const today = new Date().toISOString().split('T')[0];
    const positions = await this.positionModel.find({ portfolio_id: portfolio._id }).exec();
    let marketValue = 0;
    for (const pos of positions) {
      const price = await this.getPrice(pos.symbol);
      marketValue += Number(pos.quantity) * price;
    }
    const cashBalance = Number(portfolio.cash_balance);
    const totalEquity = cashBalance + marketValue;
    const startingCapital = Number(portfolio.starting_capital);

    const prevSnapshot = await this.snapshotModel
      .findOne({ portfolio_id: portfolio._id })
      .sort({ snapshot_date: -1 })
      .exec();
    const prevEquity = Number(prevSnapshot?.total_equity || startingCapital);
    const pnlDaily = totalEquity - prevEquity;
    const pnlCumulative = totalEquity - startingCapital;

    const peakSnapshot = await this.snapshotModel
      .findOne({ portfolio_id: portfolio._id })
      .sort({ total_equity: -1 })
      .exec();
    const peak = Math.max(Number(peakSnapshot?.total_equity || startingCapital), totalEquity);
    const drawdownPct = peak > 0 ? (peak - totalEquity) / peak : 0;

    const asiRows = await this.indexModel.find({ index_code: 'ASI' }).sort({ trade_date: -1 }).limit(2).exec();
    const benchmarkChange = asiRows.length >= 2
      ? ((Number(asiRows[0].value) - Number(asiRows[1].value)) / Number(asiRows[1].value)) * 100
      : 0;

    await this.snapshotModel.findOneAndUpdate(
      { portfolio_id: portfolio._id, snapshot_date: today },
      {
        total_equity: totalEquity,
        pnl_daily: pnlDaily,
        pnl_cumulative: pnlCumulative,
        benchmark_asi_change_pct: benchmarkChange,
        drawdown_pct: drawdownPct,
      },
      { upsert: true, new: true },
    ).exec();

    log.done({ totalEquity, pnlDaily, drawdownPct });
  }

  private async getPrice(symbol: string): Promise<number> {
    const cached = await this.redis.get(`price:${symbol}`);
    if (cached) return JSON.parse(cached).price;
    const row = await this.priceModel.findOne({ symbol }).sort({ trade_date: -1 }).exec();
    return Number(row?.price || 0);
  }
}
