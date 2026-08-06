import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';
import { IndicatorService } from './indicator.service';
import { LlmService } from './llm.service';
import { PORTFOLIO_PROMPT_VERSION, PROMPT_VERSION, TechnicalSnapshot } from '@ngx/shared';
import { EventsGateway } from '../events/events.gateway';
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
    private readonly db: DatabaseService,
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
    const indexData = await this.db.query(
      `SELECT index_code, value, week_change FROM index_history
       WHERE index_code = 'ASI' ORDER BY trade_date DESC LIMIT 1`,
    );

    const maxPicks = Number((paramSet as ParamSetRow).max_daily_trades || 5);
    const context = {
      universe,
      positions,
      marketContext: indexData.rows[0] || null,
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
    const instrument = await this.db.query('SELECT * FROM instruments WHERE symbol = $1 AND is_active = true', [symbol]);
    if (instrument.rows.length === 0) {
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

    const fundamental = await this.db.query(
      `SELECT * FROM fundamentals_snapshot WHERE symbol = $1 ORDER BY snapshot_date DESC LIMIT 1`,
      [symbol],
    );
    const news = await this.db.query(
      `SELECT headline, category, published_at FROM news
       WHERE symbol = $1 AND published_at > now() - interval '48 hours' LIMIT 5`,
      [symbol],
    );
    const indexData = await this.db.query(
      `SELECT index_code, value, week_change FROM index_history
       WHERE index_code = 'ASI' ORDER BY trade_date DESC LIMIT 1`,
    );

    const context = {
      symbol,
      technical,
      fundamental: fundamental.rows[0] || null,
      news: news.rows,
      marketContext: indexData.rows[0] || null,
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
    const fundamental = await this.db.query(
      `SELECT * FROM fundamentals_snapshot WHERE symbol = $1 ORDER BY snapshot_date DESC LIMIT 1`,
      [symbol],
    );

    const result = await this.db.query(
      `INSERT INTO signals (symbol, action, confidence, rationale, technical_snapshot, fundamental_snapshot,
        model_name, prompt_version, risk_policy_result, executed)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'BLOCKED_OTHER', false)
       RETURNING id`,
      [
        symbol,
        pick.action,
        pick.confidence,
        pick.rationale,
        JSON.stringify(technical),
        fundamental.rows[0] ? JSON.stringify(fundamental.rows[0]) : null,
        options.modelName,
        options.promptVersion,
      ],
    );
    const signalId = result.rows[0].id as string;

    await this.db.query(
      `INSERT INTO signal_llm_logs (signal_id, prompt, raw_response) VALUES ($1, $2, $3)`,
      [signalId, options.prompt, options.rawResponse],
    );

    await this.redis.publish('signals:new', JSON.stringify({ id: signalId, symbol, action: pick.action }));
    this.events.broadcastSignal({
      id: signalId,
      symbol,
      action: pick.action,
      confidence: pick.confidence,
      rationale: pick.rationale,
    });

    return signalId;
  }

  private async buildMarketUniverse(paramSet: ParamSetRow): Promise<UniverseRow[]> {
    const allowedFilter = paramSet.allowed_symbols?.length
      ? 'AND i.symbol = ANY($1::text[])'
      : '';
    const params = paramSet.allowed_symbols?.length ? [paramSet.allowed_symbols] : [];

    const result = await this.db.query(
      `SELECT i.symbol, i.name, i.sector, ph.price, ph.change_percent, ph.volume
       FROM instruments i
       LEFT JOIN LATERAL (
         SELECT price, change_percent, volume
         FROM price_history
         WHERE symbol = i.symbol
         ORDER BY trade_date DESC
         LIMIT 1
       ) ph ON true
       WHERE i.is_active = true
       ${allowedFilter}
       ORDER BY ph.volume DESC NULLS LAST, i.symbol ASC`,
      params,
    );

    return result.rows.map((row) => ({
      symbol: row.symbol as string,
      name: row.name as string | null,
      sector: row.sector as string | null,
      price: row.price != null ? Number(row.price) : null,
      change_percent: row.change_percent != null ? Number(row.change_percent) : null,
      volume: row.volume != null ? Number(row.volume) : null,
    }));
  }

  private async getPortfolioPositions(portfolioId?: string) {
    if (portfolioId) {
      const r = await this.db.query(
        `SELECT symbol, quantity, avg_cost FROM sandbox_positions WHERE portfolio_id = $1`,
        [portfolioId],
      );
      return r.rows;
    }
    const r = await this.db.query(
      `SELECT sp.symbol, sp.quantity, sp.avg_cost
       FROM sandbox_positions sp
       JOIN sandbox_portfolios p ON p.id = sp.portfolio_id
       WHERE p.name = 'default-sandbox'`,
    );
    return r.rows;
  }

  private async getActiveParamSet(portfolioId?: string) {
    if (portfolioId) {
      const r = await this.db.query(
        `SELECT s.* FROM strategy_param_sets s
         JOIN sandbox_portfolios p ON p.strategy_param_set_id = s.id
         WHERE p.id = $1`,
        [portfolioId],
      );
      if (r.rows[0]) return r.rows[0];
    }
    const r = await this.db.query(`SELECT * FROM strategy_param_sets WHERE is_active = true LIMIT 1`);
    return r.rows[0];
  }
}
