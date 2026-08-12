import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import {
  Instrument,
  InstrumentDocument,
  IndexHistory,
  IndexHistoryDocument,
  FundamentalsSnapshot,
  FundamentalsSnapshotDocument,
  News,
  NewsDocument,
  Signal,
  SignalDocument,
  SignalLlmLog,
  SignalLlmLogDocument,
  SandboxPortfolio,
  SandboxPortfolioDocument,
  SandboxPosition,
  SandboxPositionDocument,
  StrategyParamSet,
  StrategyParamSetDocument,
} from '../database/schemas';
import { RedisService } from '../redis/redis.service';
import { IndicatorService } from './indicator.service';
import { LlmService } from './llm.service';
import { PORTFOLIO_PROMPT_VERSION, PROMPT_VERSION, TechnicalSnapshot } from '@ngx/shared';
import { EventsGateway } from '../events/events.gateway';
import { docToApi } from '../database/mongo.util';
import { logStart } from '../common/log.util';

interface ParamSetRow {
  allowed_symbols: string[] | null;
  max_daily_trades: number;
}

interface UniverseRow {
  symbol: string;
  name: string | null;
  sector: string | null;
  price: number | null;
  change_percent: number | null;
  volume: number | null;
}

@Injectable()
export class SignalGenerationService {
  private readonly logger = new Logger(SignalGenerationService.name);

  constructor(
    @InjectModel(Instrument.name) private readonly instrumentModel: Model<InstrumentDocument>,
    @InjectModel(IndexHistory.name) private readonly indexModel: Model<IndexHistoryDocument>,
    @InjectModel(FundamentalsSnapshot.name) private readonly fundamentalsModel: Model<FundamentalsSnapshotDocument>,
    @InjectModel(News.name) private readonly newsModel: Model<NewsDocument>,
    @InjectModel(Signal.name) private readonly signalModel: Model<SignalDocument>,
    @InjectModel(SignalLlmLog.name) private readonly llmLogModel: Model<SignalLlmLogDocument>,
    @InjectModel(SandboxPortfolio.name) private readonly portfolioModel: Model<SandboxPortfolioDocument>,
    @InjectModel(SandboxPosition.name) private readonly positionModel: Model<SandboxPositionDocument>,
    @InjectModel(StrategyParamSet.name) private readonly paramModel: Model<StrategyParamSetDocument>,
    private readonly redis: RedisService,
    private readonly indicators: IndicatorService,
    private readonly llm: LlmService,
    private readonly events: EventsGateway,
  ) {}

  async generateForPortfolio(portfolioId?: string): Promise<string[]> {
    const log = logStart(this.logger, 'generateForPortfolio', { portfolioId });
    const paramSet = await this.getActiveParamSet(portfolioId);
    if (!paramSet) {
      log.warn('no active strategy param set');
      log.done({ signals: 0 });
      return [];
    }

    const universe = await this.buildMarketUniverse(paramSet as ParamSetRow);
    if (universe.length === 0) {
      log.warn('no tradeable symbols in universe');
      log.done({ signals: 0 });
      return [];
    }

    const positions = await this.getPortfolioPositions(portfolioId);
    const indexData = await this.indexModel.findOne({ index_code: 'ASI' }).sort({ trade_date: -1 }).exec();
    const maxPicks = Number((paramSet as ParamSetRow).max_daily_trades || 5);
    const context = {
      universe,
      positions,
      marketContext: indexData ? docToApi(indexData) : null,
      maxPicks,
    };

    const { output, prompt, rawResponse, modelName } = await this.llm.generatePortfolioSignals(context);
    const validSymbols = new Set(universe.map((row) => row.symbol));
    const signalIds: string[] = [];
    const seen = new Set<string>();

    for (const pick of output.signals) {
      if (seen.has(pick.symbol)) continue;
      seen.add(pick.symbol);
      if (!validSymbols.has(pick.symbol)) {
        log.warn('LLM picked unknown symbol', { symbol: pick.symbol });
        continue;
      }
      try {
        const signalId = await this.persistSignal(pick, {
          prompt,
          rawResponse,
          modelName,
          promptVersion: PORTFOLIO_PROMPT_VERSION,
        });
        if (signalId) signalIds.push(signalId);
      } catch (err) {
        log.fail(err);
      }
    }

    log.done({ signals: signalIds.length, universe: universe.length, picks: output.signals.length });
    return signalIds;
  }

  async generateForSymbol(symbol: string): Promise<string | null> {
    const log = logStart(this.logger, 'generateForSymbol', { symbol });
    const instrument = await this.instrumentModel.findOne({ symbol, is_active: true }).exec();
    if (!instrument) {
      log.debug('skipped', { reason: 'instrument not found' });
      log.done({ signalId: null });
      return null;
    }

    const technical = await this.indicators.compute(symbol);
    if (!technical) {
      log.debug('skipped', { reason: 'insufficient price history' });
      log.done({ signalId: null });
      return null;
    }

    const fundamental = await this.fundamentalsModel.findOne({ symbol }).sort({ snapshot_date: -1 }).exec();
    const since = new Date(Date.now() - 48 * 60 * 60 * 1000);
    const news = await this.newsModel.find({ symbol, published_at: { $gte: since } }).limit(5).exec();
    const indexData = await this.indexModel.findOne({ index_code: 'ASI' }).sort({ trade_date: -1 }).exec();

    const context = {
      symbol,
      technical,
      fundamental: fundamental ? docToApi(fundamental) : null,
      news: news.map((n) => docToApi(n)),
      marketContext: indexData ? docToApi(indexData) : null,
    };

    const { output, prompt, rawResponse, modelName } = await this.llm.generateSignal(context);
    const signalId = await this.persistSignal(output, {
      prompt,
      rawResponse,
      modelName,
      symbolOverride: symbol,
      technical,
      promptVersion: PROMPT_VERSION,
    });
    log.done({ signalId, action: output.action, confidence: output.confidence });
    return signalId;
  }

  private async persistSignal(
    pick: { symbol?: string; action: 'BUY' | 'SELL' | 'HOLD'; confidence: number; rationale: string },
    options: {
      prompt: string;
      rawResponse: string;
      modelName: string;
      symbolOverride?: string;
      technical?: TechnicalSnapshot | null;
      promptVersion: string;
    },
  ): Promise<string | null> {
    const symbol = options.symbolOverride || pick.symbol;
    if (!symbol) return null;

    const technical = options.technical ?? await this.indicators.compute(symbol);
    const fundamental = await this.fundamentalsModel.findOne({ symbol }).sort({ snapshot_date: -1 }).exec();

    const signal = await this.signalModel.create({
      symbol,
      action: pick.action,
      confidence: pick.confidence,
      rationale: pick.rationale,
      technical_snapshot: technical || {},
      fundamental_snapshot: fundamental ? docToApi(fundamental) : null,
      model_name: options.modelName,
      prompt_version: options.promptVersion,
      risk_policy_result: 'BLOCKED_OTHER',
      executed: false,
    });

    await this.llmLogModel.create({
      signal_id: signal._id,
      prompt: options.prompt,
      raw_response: options.rawResponse,
    });

    await this.redis.publish('signals:new', JSON.stringify({ id: signal._id, symbol, action: pick.action }));
    this.events.broadcastSignal({
      id: signal._id,
      symbol,
      action: pick.action,
      confidence: pick.confidence,
      rationale: pick.rationale,
    });

    return signal._id;
  }

  private async buildMarketUniverse(paramSet: ParamSetRow): Promise<UniverseRow[]> {
    const match: Record<string, unknown> = { is_active: true };
    if (paramSet.allowed_symbols?.length) {
      match.symbol = { $in: paramSet.allowed_symbols };
    }

    const rows = await this.instrumentModel.aggregate([
      { $match: match },
      {
        $lookup: {
          from: 'price_history',
          let: { sym: '$symbol' },
          pipeline: [
            { $match: { $expr: { $eq: ['$symbol', '$$sym'] } } },
            { $sort: { trade_date: -1 } },
            { $limit: 1 },
          ],
          as: 'latestPrice',
        },
      },
      { $unwind: { path: '$latestPrice', preserveNullAndEmptyArrays: true } },
      { $sort: { 'latestPrice.volume': -1, symbol: 1 } },
    ]);

    return rows.map((row) => ({
      symbol: row.symbol as string,
      name: row.name as string | null,
      sector: row.sector as string | null,
      price: row.latestPrice?.price != null ? Number(row.latestPrice.price) : null,
      change_percent: row.latestPrice?.change_percent != null ? Number(row.latestPrice.change_percent) : null,
      volume: row.latestPrice?.volume != null ? Number(row.latestPrice.volume) : null,
    }));
  }

  private async getPortfolioPositions(portfolioId?: string) {
    if (portfolioId) {
      const rows = await this.positionModel.find({ portfolio_id: portfolioId }).exec();
      return rows.map((r) => docToApi(r));
    }
    const portfolio = await this.portfolioModel.findOne({ name: 'default-sandbox' }).exec();
    if (!portfolio) return [];
    const rows = await this.positionModel.find({ portfolio_id: portfolio._id }).exec();
    return rows.map((r) => docToApi(r));
  }

  private async getActiveParamSet(portfolioId?: string) {
    if (portfolioId) {
      const portfolio = await this.portfolioModel.findById(portfolioId).exec();
      if (portfolio) {
        return this.paramModel.findById(portfolio.strategy_param_set_id).exec();
      }
    }
    return this.paramModel.findOne({ is_active: true }).exec();
  }
}
