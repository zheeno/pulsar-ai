import { Controller, Get, Logger, Query, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { InjectModel } from '@nestjs/mongoose';
import { Model } from 'mongoose';
import { SandboxTrade, SandboxTradeDocument, Signal, SignalDocument } from '../database/schemas';
import { docToApi } from '../database/mongo.util';
import { logStart } from '../common/log.util';

@Controller('trades')
@UseGuards(JwtAuthGuard)
export class TradesController {
  private readonly logger = new Logger(TradesController.name);

  constructor(
    @InjectModel(SandboxTrade.name) private readonly tradeModel: Model<SandboxTradeDocument>,
    @InjectModel(Signal.name) private readonly signalModel: Model<SignalDocument>,
  ) {}

  @Get()
  async list(@Query('page') page = '1', @Query('limit') limit = '20') {
    const log = logStart(this.logger, 'list', { page, limit });
    const offset = (Number(page) - 1) * Number(limit);
    const [trades, total] = await Promise.all([
      this.tradeModel.find().sort({ executed_at: -1 }).skip(offset).limit(Number(limit)).exec(),
      this.tradeModel.countDocuments().exec(),
    ]);

    const data = [];
    for (const trade of trades) {
      const apiTrade = docToApi(trade)!;
      if (trade.signal_id) {
        const signal = await this.signalModel.findById(trade.signal_id).exec();
        if (signal) {
          Object.assign(apiTrade, { rationale: signal.rationale, confidence: signal.confidence });
        }
      }
      data.push(apiTrade);
    }

    const result = { data, total };
    log.done({ total: result.total });
    return result;
  }
}
