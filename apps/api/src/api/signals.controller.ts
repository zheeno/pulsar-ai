import { Controller, Get, Logger, Query, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import { Signal, SignalDocument } from '../database/schemas';
import { docsToApi } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Controller('signals')
@UseGuards(JwtAuthGuard)
export class SignalsController {
  private readonly logger = new Logger(SignalsController.name);

  constructor(@InjectModel(Signal.name) private readonly signalModel: Model<SignalDocument>) {}

  @Get()
  async list(
    @Query('symbol') symbol?: string,
    @Query('action') action?: string,
    @Query('page') page = '1',
    @Query('limit') limit = '20',
  ) {
    const log = logStart(this.logger, 'list', { symbol, action, page, limit });
    const filter: Record<string, string> = {};
    if (symbol) filter.symbol = symbol;
    if (action) filter.action = action;

    const pageNum = Number(page);
    const limitNum = Number(limit);
    const offset = (pageNum - 1) * limitNum;

    const [rows, total] = await Promise.all([
      this.signalModel.find(filter).sort({ generated_at: -1 }).skip(offset).limit(limitNum).exec(),
      this.signalModel.countDocuments(filter).exec(),
    ]);

    const result = { data: docsToApi(rows), total, page: pageNum, limit: limitNum };
    log.done({ total: result.total });
    return result;
  }
}
