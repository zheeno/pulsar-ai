import { Controller, Get, Logger, Param, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { PortfolioService } from './portfolio.service';
import { logStart } from '../common/log.util';

@Controller('portfolio')
@UseGuards(JwtAuthGuard)
export class PortfolioController {
  private readonly logger = new Logger(PortfolioController.name);

  constructor(private readonly portfolio: PortfolioService) {}

  @Get('default')
  async getDefault() {
    const log = logStart(this.logger, 'getDefault');
    const id = await this.portfolio.getDefaultPortfolioId();
    if (!id) {
      log.warn('no portfolio found');
      log.done({ found: false });
      return { error: 'No portfolio found' };
    }
    const result = await this.portfolio.getPortfolio(id);
    log.done({ found: true });
    return result;
  }

  @Get(':id')
  async getPortfolio(@Param('id') id: string) {
    const log = logStart(this.logger, 'getPortfolio', { id });
    const result = await this.portfolio.getPortfolio(id);
    log.done({ found: !!result });
    return result;
  }

  @Get(':id/performance')
  async getPerformance(@Param('id') id: string) {
    const log = logStart(this.logger, 'getPerformance', { id });
    const result = await this.portfolio.getPerformance(id);
    log.done({ snapshots: result.length });
    return result;
  }
}
