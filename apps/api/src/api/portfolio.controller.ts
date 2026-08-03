import { Controller, Get, Param, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { PortfolioService } from './portfolio.service';

@Controller('portfolio')
@UseGuards(JwtAuthGuard)
export class PortfolioController {
  constructor(private readonly portfolio: PortfolioService) {}

  @Get('default')
  async getDefault() {
    const id = await this.portfolio.getDefaultPortfolioId();
    if (!id) return { error: 'No portfolio found' };
    return this.portfolio.getPortfolio(id);
  }

  @Get(':id')
  async getPortfolio(@Param('id') id: string) {
    return this.portfolio.getPortfolio(id);
  }

  @Get(':id/performance')
  async getPerformance(@Param('id') id: string) {
    return this.portfolio.getPerformance(id);
  }
}
