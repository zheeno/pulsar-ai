import { Body, Controller, Get, Logger, Param, Post, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { StrategyParamsService } from '../strategy/strategy-params.service';
import { logStart } from '../common/log.util';

@Controller('strategy-params')
@UseGuards(JwtAuthGuard)
export class StrategyParamsController {
  private readonly logger = new Logger(StrategyParamsController.name);

  constructor(private readonly service: StrategyParamsService) {}

  @Get()
  async list() {
    const log = logStart(this.logger, 'list');
    const result = await this.service.getAll();
    log.done({ count: result.length });
    return result;
  }

  @Post()
  async create(@Body() body: unknown) {
    const log = logStart(this.logger, 'create');
    const result = await this.service.create(body);
    log.done({ id: result.id });
    return result;
  }

  @Post(':id/activate')
  async activate(@Param('id') id: string) {
    const log = logStart(this.logger, 'activate', { id });
    const result = await this.service.activate(id);
    log.done({ id: result.id });
    return result;
  }
}
