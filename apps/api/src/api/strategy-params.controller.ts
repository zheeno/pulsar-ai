import { Body, Controller, Get, Param, Post, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { StrategyParamsService } from '../strategy/strategy-params.service';

@Controller('strategy-params')
@UseGuards(JwtAuthGuard)
export class StrategyParamsController {
  constructor(private readonly service: StrategyParamsService) {}

  @Get()
  async list() {
    return this.service.getAll();
  }

  @Post()
  async create(@Body() body: unknown) {
    return this.service.create(body);
  }

  @Post(':id/activate')
  async activate(@Param('id') id: string) {
    return this.service.activate(id);
  }
}
