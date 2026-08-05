import { Injectable, Logger } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { StrategyParamSetSchema } from '@ngx/shared';
import { logStart } from '../common/log.util';

@Injectable()
export class StrategyParamsService {
  private readonly logger = new Logger(StrategyParamsService.name);

  constructor(private readonly db: DatabaseService) {}

  async getActive(): Promise<Record<string, unknown> | null> {
    const log = logStart(this.logger, 'getActive');
    const r = await this.db.query(`SELECT * FROM strategy_param_sets WHERE is_active = true LIMIT 1`);
    const active = r.rows[0] || null;
    log.done({ found: !!active, id: active?.id });
    return active;
  }

  async getAll(): Promise<Record<string, unknown>[]> {
    const log = logStart(this.logger, 'getAll');
    const r = await this.db.query(`SELECT * FROM strategy_param_sets ORDER BY created_at DESC`);
    log.done({ count: r.rows.length });
    return r.rows;
  }

  async create(data: unknown): Promise<Record<string, unknown>> {
    const log = logStart(this.logger, 'create');
    const parsed = StrategyParamSetSchema.parse(data);
    const r = await this.db.query(
      `INSERT INTO strategy_param_sets (name, max_position_pct, max_daily_trades, stop_loss_pct,
        take_profit_pct, min_confidence_to_trade, max_daily_drawdown_pct, allowed_symbols, position_size_pct, is_active)
       VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, false) RETURNING *`,
      [
        parsed.name, parsed.max_position_pct, parsed.max_daily_trades, parsed.stop_loss_pct,
        parsed.take_profit_pct ?? null, parsed.min_confidence_to_trade, parsed.max_daily_drawdown_pct,
        parsed.allowed_symbols ?? null, parsed.position_size_pct ?? 0.05,
      ],
    );
    log.done({ id: r.rows[0].id, name: parsed.name });
    return r.rows[0];
  }

  async activate(id: string): Promise<Record<string, unknown>> {
    const log = logStart(this.logger, 'activate', { id });
    await this.db.query(`UPDATE strategy_param_sets SET is_active = false`);
    const r = await this.db.query(
      `UPDATE strategy_param_sets SET is_active = true WHERE id = $1 RETURNING *`,
      [id],
    );
    log.done({ id, name: r.rows[0]?.name });
    return r.rows[0];
  }
}
