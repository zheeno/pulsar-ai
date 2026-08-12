import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import {
  BacktestRun,
  BacktestRunDocument,
  StrategyParamSet,
  StrategyParamSetDocument,
  Instrument,
  InstrumentDocument,
  PriceHistory,
  PriceHistoryDocument,
  Signal,
  SignalDocument,
} from '../database/schemas';
import { IndicatorService } from '../signal-generation/indicator.service';
import { LlmService } from '../signal-generation/llm.service';
import { RiskPolicyService, ParamSet } from '../strategy/risk-policy.service';
import { FillSimulatorService } from '../execution/fill-simulator.service';
import { PROMPT_VERSION } from '@ngx/shared';
import { startOfDay, endOfDay } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Injectable()
export class BacktestService {
  private readonly logger = new Logger(BacktestService.name);

  constructor(
    @InjectModel(BacktestRun.name) private readonly backtestModel: Model<BacktestRunDocument>,
    @InjectModel(StrategyParamSet.name) private readonly paramModel: Model<StrategyParamSetDocument>,
    @InjectModel(Instrument.name) private readonly instrumentModel: Model<InstrumentDocument>,
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
    @InjectModel(Signal.name) private readonly signalModel: Model<SignalDocument>,
    private readonly indicators: IndicatorService,
    private readonly llm: LlmService,
    private readonly riskPolicy: RiskPolicyService,
    private readonly fillSimulator: FillSimulatorService,
  ) {}

  async startRun(strategyParamSetId: string, startDate: string, endDate: string): Promise<string> {
    const log = logStart(this.logger, 'startRun', { strategyParamSetId, startDate, endDate });
    const run = await this.backtestModel.create({
      strategy_param_set_id: strategyParamSetId,
      start_date: startDate,
      end_date: endDate,
      status: 'running',
    });
    const runId = run._id;
    this.runAsync(runId, strategyParamSetId, startDate, endDate).catch((err) => {
      this.logger.error(`Backtest ${runId} failed: ${err}`);
      this.backtestModel.findByIdAndUpdate(runId, { status: 'failed', completed_at: new Date() }).exec();
    });
    log.done({ runId });
    return runId;
  }

  async getRun(runId: string) {
    const log = logStart(this.logger, 'getRun', { runId });
    const run = await this.backtestModel.findById(runId).exec();
    log.done({ found: !!run, status: run?.status });
    return run ? { ...run.toObject(), id: run._id } : null;
  }

  private async runAsync(runId: string, paramSetId: string, startDate: string, endDate: string) {
    const log = logStart(this.logger, 'runAsync', { runId, startDate, endDate });
    const paramSet = await this.paramModel.findById(paramSetId).exec();
    if (!paramSet) throw new Error('Strategy param set not found');
    const symbols = await this.resolveSymbols(paramSet.toObject() as ParamSet);

    let cash = 10000000;
    const positions: Record<string, { quantity: number; avg_cost: number }> = {};
    const equityCurve: { date: string; equity: number }[] = [];
    let trades = 0;
    let wins = 0;

    const dates = await this.priceModel.distinct('trade_date', {
      trade_date: { $gte: startDate, $lte: endDate },
    });
    dates.sort();

    for (const date of dates) {
      const prices: Record<string, number> = {};
      for (const symbol of symbols) {
        const row = await this.priceModel
          .findOne({ symbol, trade_date: { $lte: date } })
          .sort({ trade_date: -1 })
          .exec();
        if (row) prices[symbol] = Number(row.price);
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
          paramSet.toObject() as ParamSet,
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
    await this.backtestModel.findByIdAndUpdate(runId, {
      status: 'completed',
      results,
      completed_at: new Date(),
    }).exec();
    log.done({ totalReturn, trades, winRate });
  }

  private async getTechnicalAtDate(symbol: string, date: string) {
    const rows = await this.priceModel
      .find({ symbol, trade_date: { $lte: date } })
      .sort({ trade_date: -1 })
      .limit(250)
      .exec();
    if (rows.length < 60) return null;
    const ordered = [...rows].reverse();
    const prices = ordered.map((r) => Number(r.price));
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
    const rows = await this.instrumentModel.find({ is_active: true }).sort({ symbol: 1 }).exec();
    return rows.map((row) => row.symbol);
  }

  private async getCachedLlm(symbol: string, date: string) {
    const dayStart = startOfDay(new Date(date));
    const dayEnd = endOfDay(new Date(date));
    const signal = await this.signalModel.findOne({
      symbol,
      prompt_version: PROMPT_VERSION,
      generated_at: { $gte: dayStart, $lte: dayEnd },
    }).exec();
    if (signal) {
      return {
        action: signal.action as 'BUY' | 'SELL' | 'HOLD',
        confidence: Number(signal.confidence),
        rationale: signal.rationale,
      };
    }
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
