import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';
import { logStart } from '../common/log.util';

@Injectable()
export class DailySnapshotService {
  private readonly logger = new Logger(DailySnapshotService.name);

  constructor(
    private readonly db: DatabaseService,
    private readonly redis: RedisService,
  ) {}

  async createSnapshot(portfolioId?: string): Promise<void> {
    const log = logStart(this.logger, 'createSnapshot', { portfolioId });
    const portfolios = portfolioId
      ? (await this.db.query('SELECT * FROM sandbox_portfolios WHERE id = $1', [portfolioId])).rows
      : (await this.db.query('SELECT * FROM sandbox_portfolios')).rows;

    for (const portfolio of portfolios) {
      await this.snapshotPortfolio(portfolio);
    }
    log.done({ portfolios: portfolios.length });
  }

  private async snapshotPortfolio(portfolio: Record<string, unknown>): Promise<void> {
    const log = logStart(this.logger, 'snapshotPortfolio', { portfolioId: portfolio.id });
    const today = new Date().toISOString().split('T')[0];
    const positions = await this.db.query('SELECT * FROM sandbox_positions WHERE portfolio_id = $1', [portfolio.id]);
    let marketValue = 0;
    for (const pos of positions.rows) {
      const price = await this.getPrice(pos.symbol as string);
      marketValue += Number(pos.quantity) * price;
    }
    const cashBalance = Number(portfolio.cash_balance);
    const totalEquity = cashBalance + marketValue;
    const startingCapital = Number(portfolio.starting_capital);

    const prevSnapshot = await this.db.query(
      `SELECT total_equity FROM daily_performance_snapshot WHERE portfolio_id = $1 ORDER BY snapshot_date DESC LIMIT 1`,
      [portfolio.id],
    );
    const prevEquity = Number(prevSnapshot.rows[0]?.total_equity || startingCapital);
    const pnlDaily = totalEquity - prevEquity;
    const pnlCumulative = totalEquity - startingCapital;

    const peakResult = await this.db.query(
      `SELECT MAX(total_equity) as peak FROM daily_performance_snapshot WHERE portfolio_id = $1`,
      [portfolio.id],
    );
    const peak = Math.max(Number(peakResult.rows[0]?.peak || startingCapital), totalEquity);
    const drawdownPct = peak > 0 ? (peak - totalEquity) / peak : 0;

    const asiResult = await this.db.query(
      `SELECT value, week_change FROM index_history WHERE index_code = 'ASI' ORDER BY trade_date DESC LIMIT 2`,
    );
    const benchmarkChange = asiResult.rows.length >= 2
      ? ((Number(asiResult.rows[0].value) - Number(asiResult.rows[1].value)) / Number(asiResult.rows[1].value)) * 100
      : 0;

    await this.db.query(
      `INSERT INTO daily_performance_snapshot (portfolio_id, snapshot_date, total_equity, pnl_daily, pnl_cumulative, benchmark_asi_change_pct, drawdown_pct)
       VALUES ($1, $2, $3, $4, $5, $6, $7)
       ON CONFLICT (portfolio_id, snapshot_date) DO UPDATE SET
         total_equity = EXCLUDED.total_equity, pnl_daily = EXCLUDED.pnl_daily,
         pnl_cumulative = EXCLUDED.pnl_cumulative, drawdown_pct = EXCLUDED.drawdown_pct`,
      [portfolio.id, today, totalEquity, pnlDaily, pnlCumulative, benchmarkChange, drawdownPct],
    );
    log.done({ totalEquity, pnlDaily, drawdownPct });
  }

  private async getPrice(symbol: string): Promise<number> {
    const cached = await this.redis.get(`price:${symbol}`);
    if (cached) return JSON.parse(cached).price;
    const r = await this.db.query('SELECT price FROM price_history WHERE symbol = $1 ORDER BY trade_date DESC LIMIT 1', [symbol]);
    return Number(r.rows[0]?.price || 0);
  }
}
