import { Injectable, Logger } from '@nestjs/common';
import { InjectConnection, InjectModel } from '@nestjs/mongoose';
import { Connection, Model } from 'mongoose';
import {
  Signal,
  SignalDocument,
  SandboxPortfolio,
  SandboxPortfolioDocument,
  SandboxPosition,
  SandboxPositionDocument,
  SandboxTrade,
  SandboxTradeDocument,
  StrategyParamSet,
  StrategyParamSetDocument,
  PriceHistory,
  PriceHistoryDocument,
  DailyPerformanceSnapshot,
  DailyPerformanceSnapshotDocument,
} from '../database/schemas';
import { RedisService } from '../redis/redis.service';
import { RiskPolicyService, ParamSet } from '../strategy/risk-policy.service';
import { FillSimulatorService } from './fill-simulator.service';
import { EventsGateway } from '../events/events.gateway';
import { docToApi } from '../database/mongo.util';
import { startOfDay, endOfDay } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Injectable()
export class ExecutionService {
  private readonly logger = new Logger(ExecutionService.name);

  constructor(
    @InjectConnection() private readonly connection: Connection,
    @InjectModel(Signal.name) private readonly signalModel: Model<SignalDocument>,
    @InjectModel(SandboxPortfolio.name) private readonly portfolioModel: Model<SandboxPortfolioDocument>,
    @InjectModel(SandboxPosition.name) private readonly positionModel: Model<SandboxPositionDocument>,
    @InjectModel(SandboxTrade.name) private readonly tradeModel: Model<SandboxTradeDocument>,
    @InjectModel(StrategyParamSet.name) private readonly paramModel: Model<StrategyParamSetDocument>,
    @InjectModel(PriceHistory.name) private readonly priceModel: Model<PriceHistoryDocument>,
    @InjectModel(DailyPerformanceSnapshot.name) private readonly snapshotModel: Model<DailyPerformanceSnapshotDocument>,
    private readonly redis: RedisService,
    private readonly riskPolicy: RiskPolicyService,
    private readonly fillSimulator: FillSimulatorService,
    private readonly events: EventsGateway,
  ) {}

  async processSignals(signalIds: string[]): Promise<number> {
    const log = logStart(this.logger, 'processSignals', { count: signalIds.length });
    let executed = 0;
    for (const signalId of signalIds) {
      try {
        const didExecute = await this.processSignal(signalId);
        if (didExecute) executed++;
      } catch (err) {
        log.fail(err);
      }
    }
    log.done({ executed, total: signalIds.length });
    return executed;
  }

  async processSignal(signalId: string): Promise<boolean> {
    const log = logStart(this.logger, 'processSignal', { signalId });
    const signal = await this.signalModel.findById(signalId).exec();
    if (!signal) {
      log.debug('skipped', { reason: 'signal not found' });
      log.done({ executed: false });
      return false;
    }

    const portfolio = await this.portfolioModel.findOne({ name: 'default-sandbox' }).exec();
    if (!portfolio) {
      log.debug('skipped', { reason: 'portfolio not found' });
      log.done({ executed: false });
      return false;
    }

    const paramSet = await this.paramModel.findById(portfolio.strategy_param_set_id).exec();
    if (!paramSet) {
      log.debug('skipped', { reason: 'param set not found' });
      log.done({ executed: false });
      return false;
    }

    const positions = await this.positionModel.find({ portfolio_id: portfolio._id }).exec();
    const prices = await this.getCurrentPrices(
      positions.map((p) => p.symbol).concat([signal.symbol]),
    );

    const dailyTrades = await this.tradeModel.countDocuments({
      portfolio_id: portfolio._id,
      executed_at: { $gte: startOfDay(), $lte: endOfDay() },
    }).exec();

    const latestSnapshot = await this.snapshotModel
      .findOne({ portfolio_id: portfolio._id })
      .sort({ snapshot_date: -1 })
      .exec();
    const dailyDrawdownPct = Number(latestSnapshot?.drawdown_pct || 0);

    const signalApi = docToApi(signal)!;
    const portfolioApi = docToApi(portfolio)!;
    const positionsApi = positions.map((p) => docToApi(p)!);

    const { result, quantity } = await this.riskPolicy.evaluate(
      {
        id: signalApi.id as string,
        symbol: signal.symbol,
        action: signal.action as 'BUY' | 'SELL' | 'HOLD',
        confidence: Number(signal.confidence),
      },
      paramSet.toObject() as ParamSet,
      { cash_balance: Number(portfolio.cash_balance), id: portfolio._id },
      positionsApi as { symbol: string; quantity: number; avg_cost: number }[],
      prices,
      dailyTrades,
      dailyDrawdownPct,
    );

    await this.signalModel.findByIdAndUpdate(signalId, { risk_policy_result: result }).exec();

    if (result !== 'APPROVED' || quantity <= 0) {
      log.done({ executed: false, riskPolicyResult: result });
      return false;
    }

    const currentPrice = prices[signal.symbol];
    if (!currentPrice) {
      log.debug('skipped', { reason: 'no current price', symbol: signal.symbol });
      log.done({ executed: false });
      return false;
    }

    const session = await this.connection.startSession();
    try {
      await session.withTransaction(async () => {
        await this.executeTrade(session, portfolio, signal, quantity, currentPrice);
      });
    } finally {
      await session.endSession();
    }

    await this.signalModel.findByIdAndUpdate(signalId, { executed: true }).exec();
    await this.redis.publish('trades:new', JSON.stringify({ signalId, symbol: signal.symbol }));
    this.events.broadcastTrade({ signalId, symbol: signal.symbol, side: signal.action, quantity });
    log.done({ executed: true, symbol: signal.symbol, side: signal.action, quantity });
    return true;
  }

  private async executeTrade(
    session: import('mongoose').ClientSession,
    portfolio: SandboxPortfolioDocument,
    signal: SignalDocument,
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

      const pos = await this.positionModel.findOne({
        portfolio_id: portfolio._id,
        symbol: signal.symbol,
      }).session(session).exec();

      if (pos) {
        const newQty = Number(pos.quantity) + quantity;
        const newAvg = (Number(pos.avg_cost) * Number(pos.quantity) + fillPrice * quantity) / newQty;
        await this.positionModel.findByIdAndUpdate(
          pos._id,
          { quantity: newQty, avg_cost: newAvg, updated_at: new Date() },
          { session },
        ).exec();
      } else {
        await this.positionModel.create([{
          portfolio_id: portfolio._id,
          symbol: signal.symbol,
          quantity,
          avg_cost: fillPrice,
        }], { session });
      }
    } else {
      const pos = await this.positionModel.findOne({
        portfolio_id: portfolio._id,
        symbol: signal.symbol,
      }).session(session).exec();
      if (!pos) throw new Error(`No position to sell for ${signal.symbol}`);
      cashBalance += notional - fee;
      const newQty = Number(pos.quantity) - quantity;
      if (newQty <= 0) {
        await this.positionModel.findByIdAndDelete(pos._id, { session }).exec();
      } else {
        await this.positionModel.findByIdAndUpdate(pos._id, { quantity: newQty, updated_at: new Date() }, { session }).exec();
      }
    }

    await this.portfolioModel.findByIdAndUpdate(
      portfolio._id,
      { cash_balance: cashBalance },
      { session },
    ).exec();

    await this.tradeModel.create([{
      portfolio_id: portfolio._id,
      signal_id: signal._id,
      symbol: signal.symbol,
      side,
      quantity,
      fill_price: fillPrice,
      simulated_fee: fee,
      simulated_slippage_bps: slippageBps,
      resulting_cash_balance: cashBalance,
    }], { session });

    portfolio.cash_balance = cashBalance;
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
      const row = await this.priceModel.findOne({ symbol }).sort({ trade_date: -1 }).exec();
      if (row) prices[symbol] = Number(row.price);
    }
    return prices;
  }
}
