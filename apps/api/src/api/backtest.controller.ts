import { Body, Controller, Get, Logger, Param, Post, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { BacktestService } from '../backtest/backtest.service';
import { BacktestRequestSchema } from '@ngx/shared';
import { logStart } from '../common/log.util';

@Controller('backtest')
@UseGuards(JwtAuthGuard)
export class BacktestController {
  private readonly logger = new Logger(BacktestController.name);

  constructor(private readonly backtest: BacktestService) {}

  @Post()
  async start(@Body() body: unknown) {
    const log = logStart(this.logger, 'start');
    const parsed = BacktestRequestSchema.parse(body);
    const runId = await this.backtest.startRun(parsed.strategy_param_set_id, parsed.start_date, parsed.end_date);
    log.done({ runId });
    return { runId, status: 'running' };
  }

  @Get(':runId')
  async get(@Param('runId') runId: string) {
    const log = logStart(this.logger, 'get', { runId });
    const result = await this.backtest.getRun(runId);
    log.done({ found: !!result });
    return result;
  }
}
