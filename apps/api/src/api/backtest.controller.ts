import { Body, Controller, Get, Param, Post, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { BacktestService } from '../backtest/backtest.service';
import { BacktestRequestSchema } from '@ngx/shared';

@Controller('backtest')
@UseGuards(JwtAuthGuard)
export class BacktestController {
  constructor(private readonly backtest: BacktestService) {}

  @Post()
  async start(@Body() body: unknown) {
    const parsed = BacktestRequestSchema.parse(body);
    const runId = await this.backtest.startRun(parsed.strategy_param_set_id, parsed.start_date, parsed.end_date);
    return { runId, status: 'running' };
  }

  @Get(':runId')
  async get(@Param('runId') runId: string) {
    return this.backtest.getRun(runId);
  }
}
