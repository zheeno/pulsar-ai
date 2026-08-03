import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { RedisService } from '../redis/redis.service';
import { IndicatorService } from './indicator.service';
import { LlmService } from './llm.service';
import { PROMPT_VERSION } from '@ngx/shared';
import { EventsGateway } from '../events/events.gateway';

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
    const paramSet = await this.getActiveParamSet(portfolioId) as { allowed_symbols: string[] | null };
    const symbols = paramSet.allowed_symbols || [];
    const signalIds: string[] = [];

    for (const symbol of symbols) {
      try {
        const signalId = await this.generateForSymbol(symbol, paramSet);
        if (signalId) signalIds.push(signalId);
      } catch (err) {
        this.logger.error(`Signal generation failed for ${symbol}: ${err}`);
      }
    }
    return signalIds;
  }

  async generateForSymbol(symbol: string, paramSet?: { allowed_symbols: string[] | null }): Promise<string | null> {
    const instrument = await this.db.query('SELECT * FROM instruments WHERE symbol = $1 AND is_active = true', [symbol]);
    if (instrument.rows.length === 0) return null;

    const technical = await this.indicators.compute(symbol);
    if (!technical) {
      this.logger.debug(`Insufficient price history for ${symbol}`);
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

    const result = await this.db.query(
      `INSERT INTO signals (symbol, action, confidence, rationale, technical_snapshot, fundamental_snapshot,
        model_name, prompt_version, risk_policy_result, executed)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, 'BLOCKED_OTHER', false)
       RETURNING id`,
      [
        symbol, output.action, output.confidence, output.rationale,
        JSON.stringify(technical), fundamental.rows[0] ? JSON.stringify(fundamental.rows[0]) : null,
        modelName, PROMPT_VERSION,
      ],
    );
    const signalId = result.rows[0].id as string;

    await this.db.query(
      `INSERT INTO signal_llm_logs (signal_id, prompt, raw_response) VALUES ($1, $2, $3)`,
      [signalId, prompt, rawResponse],
    );

    await this.redis.publish('signals:new', JSON.stringify({ id: signalId, symbol, action: output.action }));
    this.events.broadcastSignal({ id: signalId, symbol, action: output.action, confidence: output.confidence, rationale: output.rationale });

    return signalId;
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
