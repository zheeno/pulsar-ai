import { Injectable } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';

@Injectable()
export class PortfolioService {
  constructor(
    private readonly db: DatabaseService,
    private readonly redis: RedisService,
  ) {}

  async getPortfolio(id: string) {
    const portfolio = await this.db.query('SELECT * FROM sandbox_portfolios WHERE id = $1', [id]);
    if (!portfolio.rows[0]) return null;

    const positions = await this.db.query('SELECT * FROM sandbox_positions WHERE portfolio_id = $1', [id]);
    let marketValue = 0;
    const enrichedPositions = [];
    for (const pos of positions.rows) {
      const price = await this.getPrice(pos.symbol as string);
      const value = Number(pos.quantity) * price;
      marketValue += value;
      enrichedPositions.push({ ...pos, current_price: price, market_value: value });
    }

    const cashBalance = Number(portfolio.rows[0].cash_balance);
    const totalEquity = cashBalance + marketValue;

    const today = new Date().toISOString().split('T')[0];
    const todaySnapshot = await this.db.query(
      `SELECT pnl_daily FROM daily_performance_snapshot WHERE portfolio_id = $1 AND snapshot_date = $2`,
      [id, today],
    );

    return {
      portfolio: portfolio.rows[0],
      positions: enrichedPositions,
      total_equity: totalEquity,
      market_value: marketValue,
      pnl_today: Number(todaySnapshot.rows[0]?.pnl_daily || 0),
    };
  }

  async getPerformance(id: string) {
    const r = await this.db.query(
      `SELECT snapshot_date, total_equity, pnl_daily, pnl_cumulative, benchmark_asi_change_pct, drawdown_pct
       FROM daily_performance_snapshot WHERE portfolio_id = $1 ORDER BY snapshot_date`,
      [id],
    );
    return r.rows;
  }

  async getDefaultPortfolioId(): Promise<string | null> {
    const r = await this.db.query(`SELECT id FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1`);
    return r.rows[0]?.id || null;
  }

  private async getPrice(symbol: string): Promise<number> {
    const cached = await this.redis.get(`price:${symbol}`);
    if (cached) return JSON.parse(cached).price;
    const r = await this.db.query('SELECT price FROM price_history WHERE symbol = $1 ORDER BY trade_date DESC LIMIT 1', [symbol]);
    return Number(r.rows[0]?.price || 0);
  }
}
