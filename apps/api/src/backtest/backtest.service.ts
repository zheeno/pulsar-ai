import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { IndicatorService } from '../signal-generation/indicator.service';
import { LlmService } from '../signal-generation/llm.service';
import { RiskPolicyService, ParamSet } from '../strategy/risk-policy.service';
import { FillSimulatorService } from '../execution/fill-simulator.service';
import { PROMPT_VERSION } from '@ngx/shared';
import { logStart } from '../common/log.util';

@Injectable()
export class BacktestService {
  private readonly logger = new Logger(BacktestService.name);

  constructor(
    private readonly db: DatabaseService,
    private readonly indicators: IndicatorService,
    private readonly llm: LlmService,
    private readonly riskPolicy: RiskPolicyService,
    private readonly fillSimulator: FillSimulatorService,
  ) {}

  async startRun(strategyParamSetId: string, startDate: string, endDate: string): Promise<string> {
    const log = logStart(this.logger, 'startRun', { strategyParamSetId, startDate, endDate });
    const result = await this.db.query(
      `INSERT INTO backtest_runs (strategy_param_set_id, start_date, end_date, status)
       VALUES ($1, $2, $3, 'running') RETURNING id`,
      [strategyParamSetId, startDate, endDate],
    );
    const runId = result.rows[0].id as string;
    this.runAsync(runId, strategyParamSetId, startDate, endDate).catch((err) => {
      this.logger.error(`Backtest ${runId} failed: ${err}`);
      this.db.query(`UPDATE backtest_runs SET status = 'failed', completed_at = now() WHERE id = $1`, [runId]);
    });
    log.done({ runId });
    return runId;
  }

  async getRun(runId: string) {
    const log = logStart(this.logger, 'getRun', { runId });
    const r = await this.db.query('SELECT * FROM backtest_runs WHERE id = $1', [runId]);
    const run = r.rows[0] || null;
    log.done({ found: !!run, status: run?.status });
    return run;
  }

  private async runAsync(runId: string, paramSetId: string, startDate: string, endDate: string) {
    const log = logStart(this.logger, 'runAsync', { runId, startDate, endDate });
    const paramResult = await this.db.query('SELECT * FROM strategy_param_sets WHERE id = $1', [paramSetId]);
    const paramSet = paramResult.rows[0] as ParamSet;
    const symbols = await this.resolveSymbols(paramSet);

    let cash = 10000000;
    const positions: Record<string, { quantity: number; avg_cost: number }> = {};
    const equityCurve: { date: string; equity: number }[] = [];
    let trades = 0;
    let wins = 0;

    const datesResult = await this.db.query(
      `SELECT DISTINCT trade_date FROM price_history
       WHERE trade_date BETWEEN $1 AND $2 ORDER BY trade_date`,
      [startDate, endDate],
    );

    for (const row of datesResult.rows) {
      const date = row.trade_date as string;
      const prices: Record<string, number> = {};
      for (const symbol of symbols) {
        const pr = await this.db.query(
          'SELECT price FROM price_history WHERE symbol = $1 AND trade_date <= $2 ORDER BY trade_date DESC LIMIT 1',
          [symbol, date],
        );
        if (pr.rows[0]) prices[symbol] = Number(pr.rows[0].price);
      }

      for (const symbol of symbols) {
        if (!prices[symbol]) continue;
        const technical = await this.getTechnicalAtDate(symbol, date);
        if (!technical) continue;

        const context = { symbol, technical, date };
        const cached = await this.getCachedLlm(symbol, date);
        const output = cached || (await this.llm.generateSignal(context)).output;

        if (output.action === 'HOLD') continue;

        const posList = Object.entries(positions).map(([s, p]) => ({ symbol: s, ...p }));
        const marketValue = posList.reduce((sum, p) => sum + p.quantity * (prices[p.symbol] || p.avg_cost), 0);
        const totalEquity = cash + marketValue;

        const { result, quantity } = await this.riskPolicy.evaluate(
          { id: 'backtest', symbol, action: output.action, confidence: output.confidence },
          paramSet,
          { cash_balance: cash, id: 'backtest' },
          posList,
          prices,
          trades,
          0,
        );

        if (result !== 'APPROVED' || quantity <= 0) continue;

        const { fillPrice } = this.fillSimulator.simulateFill(output.action, prices[symbol]);
        const notional = fillPrice * quantity;
        const fee = this.fillSimulator.calculateFee(notional);

        if (output.action === 'BUY' && cash >= notional + fee) {
          cash -= notional + fee;
          const pos = positions[symbol] || { quantity: 0, avg_cost: 0 };
          const newQty = pos.quantity + quantity;
          pos.avg_cost = (pos.avg_cost * pos.quantity + fillPrice * quantity) / newQty;
          pos.quantity = newQty;
          positions[symbol] = pos;
          trades++;
        } else if (output.action === 'SELL' && positions[symbol]) {
          const cost = positions[symbol].avg_cost * quantity;
          cash += notional - fee;
          if (notional > cost) wins++;
          positions[symbol].quantity -= quantity;
          if (positions[symbol].quantity <= 0) delete positions[symbol];
          trades++;
        }
      }

      const marketValue = Object.entries(positions).reduce(
        (sum, [s, p]) => sum + p.quantity * (prices[s] || p.avg_cost), 0,
      );
      equityCurve.push({ date, equity: cash + marketValue });
    }

    const finalEquity = equityCurve[equityCurve.length - 1]?.equity || cash;
    const totalReturn = ((finalEquity - 10000000) / 10000000) * 100;
    const maxDrawdown = this.calculateMaxDrawdown(equityCurve.map((e) => e.equity));
    const winRate = trades > 0 ? (wins / trades) * 100 : 0;

    const results = { totalReturn, maxDrawdown, winRate, trades, equityCurve, finalEquity };
    await this.db.query(
      `UPDATE backtest_runs SET status = 'completed', results = $1, completed_at = now() WHERE id = $2`,
      [JSON.stringify(results), runId],
    );
    log.done({ totalReturn, trades, winRate });
  }

  private async getTechnicalAtDate(symbol: string, date: string) {
    const result = await this.db.query(
      `SELECT trade_date, price, volume FROM price_history
       WHERE symbol = $1 AND trade_date <= $2 ORDER BY trade_date DESC LIMIT 250`,
      [symbol, date],
    );
    if (result.rows.length < 60) return null;
    const rows = result.rows.reverse();
    const prices = rows.map((r) => Number(r.price));
    const currentPrice = prices[prices.length - 1];
    const momentum = prices.length > 20
      ? ((currentPrice - prices[prices.length - 21]) / prices[prices.length - 21]) * 100
      : 0;
    return { currentPrice, momentum, rsi14: 50, sma50: currentPrice, sma200: currentPrice, volumeAnomaly: 1 };
  }

  private async resolveSymbols(paramSet: ParamSet): Promise<string[]> {
    if (paramSet.allowed_symbols && paramSet.allowed_symbols.length > 0) {
      return paramSet.allowed_symbols;
    }
    const r = await this.db.query('SELECT symbol FROM instruments WHERE is_active = true ORDER BY symbol');
    return r.rows.map((row) => row.symbol as string);
  }

  private async getCachedLlm(symbol: string, date: string) {
    const r = await this.db.query(
      `SELECT s.action, s.confidence, s.rationale FROM signals s
       WHERE s.symbol = $1 AND s.generated_at::date = $2 AND s.prompt_version = $3 LIMIT 1`,
      [symbol, date, PROMPT_VERSION],
    );
    if (r.rows[0]) return r.rows[0] as { action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string };
    return null;
  }

  private calculateMaxDrawdown(equities: number[]): number {
    let peak = equities[0] || 0;
    let maxDd = 0;
    for (const eq of equities) {
      if (eq > peak) peak = eq;
      const dd = peak > 0 ? (peak - eq) / peak : 0;
      if (dd > maxDd) maxDd = dd;
    }
    return maxDd * 100;
  }
}
