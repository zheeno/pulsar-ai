import { Injectable, Logger } from '@nestjs/common';
import { PoolClient } from 'pg';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';
import { RiskPolicyService, ParamSet } from '../strategy/risk-policy.service';
import { FillSimulatorService } from './fill-simulator.service';
import { EventsGateway } from '../events/events.gateway';

@Injectable()
export class ExecutionService {
  private readonly logger = new Logger(ExecutionService.name);

  constructor(
    private readonly db: DatabaseService,
    private readonly redis: RedisService,
    private readonly riskPolicy: RiskPolicyService,
    private readonly fillSimulator: FillSimulatorService,
    private readonly events: EventsGateway,
  ) {}

  async processSignals(signalIds: string[]): Promise<number> {
    let executed = 0;
    for (const signalId of signalIds) {
      try {
        const didExecute = await this.processSignal(signalId);
        if (didExecute) executed++;
      } catch (err) {
        this.logger.error(`Execution failed for signal ${signalId}: ${err}`);
      }
    }
    return executed;
  }

  async processSignal(signalId: string): Promise<boolean> {
    const signalResult = await this.db.query('SELECT * FROM signals WHERE id = $1', [signalId]);
    const signal = signalResult.rows[0];
    if (!signal) return false;

    const portfolioResult = await this.db.query(`SELECT * FROM sandbox_portfolios WHERE name = 'default-sandbox' LIMIT 1`);
    const portfolio = portfolioResult.rows[0];
    if (!portfolio) return false;

    const paramResult = await this.db.query('SELECT * FROM strategy_param_sets WHERE id = $1', [portfolio.strategy_param_set_id]);
    const paramSet = paramResult.rows[0] as ParamSet;
    const portfolioTyped = portfolio as { cash_balance: number; id: string };

    const positionsResult = await this.db.query('SELECT * FROM sandbox_positions WHERE portfolio_id = $1', [portfolio.id]);
    const positions = positionsResult.rows as { symbol: string; quantity: number; avg_cost: number }[];

    const prices = await this.getCurrentPrices(positions.map((p) => p.symbol as string).concat([signal.symbol as string]));

    const today = new Date().toISOString().split('T')[0];
    const tradesToday = await this.db.query(
      `SELECT COUNT(*) as count FROM sandbox_trades WHERE portfolio_id = $1 AND executed_at::date = $2`,
      [portfolio.id, today],
    );
    const dailyTrades = Number(tradesToday.rows[0].count);

    const snapshot = await this.db.query(
      `SELECT drawdown_pct FROM daily_performance_snapshot WHERE portfolio_id = $1 ORDER BY snapshot_date DESC LIMIT 1`,
      [portfolio.id],
    );
    const dailyDrawdownPct = Number(snapshot.rows[0]?.drawdown_pct || 0);

    const { result, quantity } = await this.riskPolicy.evaluate(
      { id: signal.id, symbol: signal.symbol, action: signal.action, confidence: Number(signal.confidence) },
      paramSet,
      portfolioTyped,
      positions,
      prices,
      dailyTrades,
      dailyDrawdownPct,
    );

    await this.db.query('UPDATE signals SET risk_policy_result = $1 WHERE id = $2', [result, signalId]);

    if (result !== 'APPROVED' || quantity <= 0) return false;

    const currentPrice = prices[signal.symbol as string];
    if (!currentPrice) return false;

    await this.db.transaction(async (client) => {
      await this.executeTrade(client, portfolio, signal, quantity, currentPrice);
    });

    await this.db.query('UPDATE signals SET executed = true WHERE id = $1', [signalId]);
    await this.redis.publish('trades:new', JSON.stringify({ signalId, symbol: signal.symbol }));
    this.events.broadcastTrade({ signalId, symbol: signal.symbol, side: signal.action, quantity });
    return true;
  }

  private async executeTrade(
    client: PoolClient,
    portfolio: Record<string, unknown>,
    signal: Record<string, unknown>,
    quantity: number,
    currentPrice: number,
  ): Promise<void> {
    const side = signal.action as 'BUY' | 'SELL';
    const { fillPrice, slippageBps } = this.fillSimulator.simulateFill(side, currentPrice);
    const notional = fillPrice * quantity;
    const fee = this.fillSimulator.calculateFee(notional);
    let cashBalance = Number(portfolio.cash_balance);

    if (side === 'BUY') {
      const totalCost = notional + fee;
      if (cashBalance < totalCost) throw new Error('Insufficient cash');
      cashBalance -= totalCost;

      const posResult = await client.query(
        'SELECT * FROM sandbox_positions WHERE portfolio_id = $1 AND symbol = $2',
        [portfolio.id, signal.symbol],
      );
      if (posResult.rows.length > 0) {
        const pos = posResult.rows[0];
        const newQty = Number(pos.quantity) + quantity;
        const newAvg = (Number(pos.avg_cost) * Number(pos.quantity) + fillPrice * quantity) / newQty;
        await client.query(
          'UPDATE sandbox_positions SET quantity = $1, avg_cost = $2, updated_at = now() WHERE id = $3',
          [newQty, newAvg, pos.id],
        );
      } else {
        await client.query(
          'INSERT INTO sandbox_positions (portfolio_id, symbol, quantity, avg_cost) VALUES ($1, $2, $3, $4)',
          [portfolio.id, signal.symbol, quantity, fillPrice],
        );
      }
    } else {
      const posResult = await client.query(
        'SELECT * FROM sandbox_positions WHERE portfolio_id = $1 AND symbol = $2',
        [portfolio.id, signal.symbol],
      );
      const pos = posResult.rows[0];
      if (!pos) throw new Error(`No position to sell for ${signal.symbol}`);
      cashBalance += notional - fee;
      const newQty = Number(pos.quantity) - quantity;
      if (newQty <= 0) {
        await client.query('DELETE FROM sandbox_positions WHERE id = $1', [pos.id]);
      } else {
        await client.query('UPDATE sandbox_positions SET quantity = $1, updated_at = now() WHERE id = $2', [newQty, pos.id]);
      }
    }

    await client.query('UPDATE sandbox_portfolios SET cash_balance = $1 WHERE id = $2', [cashBalance, portfolio.id]);
    await client.query(
      `INSERT INTO sandbox_trades (portfolio_id, signal_id, symbol, side, quantity, fill_price, simulated_fee, simulated_slippage_bps, resulting_cash_balance)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)`,
      [portfolio.id, signal.id, signal.symbol, side, quantity, fillPrice, fee, slippageBps, cashBalance],
    );
  }

  private async getCurrentPrices(symbols: string[]): Promise<Record<string, number>> {
    const unique = [...new Set(symbols)];
    const prices: Record<string, number> = {};
    for (const symbol of unique) {
      const cached = await this.redis.get(`price:${symbol}`);
      if (cached) {
        prices[symbol] = JSON.parse(cached).price;
        continue;
      }
      const r = await this.db.query(
        'SELECT price FROM price_history WHERE symbol = $1 ORDER BY trade_date DESC LIMIT 1',
        [symbol],
      );
      if (r.rows[0]) prices[symbol] = Number(r.rows[0].price);
    }
    return prices;
  }
}
