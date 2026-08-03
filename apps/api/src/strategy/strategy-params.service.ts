import { Injectable } from '@nestjs/common';
import { DatabaseService } from '../database/database.service';
import { StrategyParamSetSchema } from '@ngx/shared';

@Injectable()
export class StrategyParamsService {
  constructor(private readonly db: DatabaseService) {}

  async getActive(): Promise<Record<string, unknown> | null> {
    const r = await this.db.query(`SELECT * FROM strategy_param_sets WHERE is_active = true LIMIT 1`);
    return r.rows[0] || null;
  }

  async getAll(): Promise<Record<string, unknown>[]> {
    const r = await this.db.query(`SELECT * FROM strategy_param_sets ORDER BY created_at DESC`);
    return r.rows;
  }

  async create(data: unknown): Promise<Record<string, unknown>> {
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
    return r.rows[0];
  }

  async activate(id: string): Promise<Record<string, unknown>> {
    await this.db.query(`UPDATE strategy_param_sets SET is_active = false`);
    const r = await this.db.query(
      `UPDATE strategy_param_sets SET is_active = true WHERE id = $1 RETURNING *`,
      [id],
    );
    return r.rows[0];
  }
}
