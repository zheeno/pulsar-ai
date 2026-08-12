import { Injectable, Logger } from '@nestjs/common';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import { StrategyParamSetSchema } from '@ngx/shared';
import { StrategyParamSet, StrategyParamSetDocument } from '../database/schemas';
import { docToApi, docsToApi } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Injectable()
export class StrategyParamsService {
  private readonly logger = new Logger(StrategyParamsService.name);

  constructor(
    @InjectModel(StrategyParamSet.name) private readonly paramModel: Model<StrategyParamSetDocument>,
  ) {}

  async getActive(): Promise<Record<string, unknown> | null> {
    const log = logStart(this.logger, 'getActive');
    const active = await this.paramModel.findOne({ is_active: true }).exec();
    const result = docToApi(active);
    log.done({ found: !!result, id: result?.id });
    return result;
  }

  async getAll(): Promise<Record<string, unknown>[]> {
    const log = logStart(this.logger, 'getAll');
    const rows = await this.paramModel.find().sort({ created_at: -1 }).exec();
    log.done({ count: rows.length });
    return docsToApi(rows);
  }

  async create(data: unknown): Promise<Record<string, unknown>> {
    const log = logStart(this.logger, 'create');
    const parsed = StrategyParamSetSchema.parse(data);
    const created = await this.paramModel.create({
      name: parsed.name,
      max_position_pct: parsed.max_position_pct,
      max_daily_trades: parsed.max_daily_trades,
      stop_loss_pct: parsed.stop_loss_pct,
      take_profit_pct: parsed.take_profit_pct ?? null,
      min_confidence_to_trade: parsed.min_confidence_to_trade,
      max_daily_drawdown_pct: parsed.max_daily_drawdown_pct,
      allowed_symbols: parsed.allowed_symbols ?? null,
      position_size_pct: parsed.position_size_pct ?? 0.05,
      is_active: false,
    });
    log.done({ id: created._id, name: parsed.name });
    return docToApi(created)!;
  }

  async activate(id: string): Promise<Record<string, unknown>> {
    const log = logStart(this.logger, 'activate', { id });
    await this.paramModel.updateMany({}, { is_active: false }).exec();
    const updated = await this.paramModel.findByIdAndUpdate(id, { is_active: true }, { new: true }).exec();
    log.done({ id, name: updated?.name });
    return docToApi(updated)!;
  }
}
