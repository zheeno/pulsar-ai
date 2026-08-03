import { Body, Controller, Get, Post } from '@nestjs/common';
import { AuthService } from '../auth/auth.service';
import { IngestionService } from '../market-data/ingestion.service';
import { SignalGenerationService } from '../signal-generation/signal-generation.service';
import { ExecutionService } from '../execution/execution.service';
import { DailySnapshotService } from '../orchestrator/daily-snapshot.service';
import { RateLimitService } from '../market-data/rate-limit.service';
import { PortfolioService } from './portfolio.service';

@Controller()
export class HealthController {
  constructor(
    private readonly ingestion: IngestionService,
    private readonly signals: SignalGenerationService,
    private readonly execution: ExecutionService,
    private readonly snapshot: DailySnapshotService,
    private readonly portfolio: PortfolioService,
  ) {}

  @Get('health')
  health() {
    return { status: 'ok', timestamp: new Date().toISOString() };
  }

  @Post('cycle/run')
  async runCycle() {
    const ingested = await this.ingestion.ingestStocks();
    await this.ingestion.ingestMarket();
    await this.ingestion.ingestIndices();
    const signalIds = await this.signals.generateForPortfolio();
    const executed = await this.execution.processSignals(signalIds);
    await this.snapshot.createSnapshot();
    const portfolioId = await this.portfolio.getDefaultPortfolioId();
    const portfolio = portfolioId ? await this.portfolio.getPortfolio(portfolioId) : null;
    return { ingested, signals: signalIds.length, executed, portfolio };
  }
}

@Controller('auth')
export class AuthController {
  constructor(private readonly auth: AuthService) {}

  @Post('login')
  async login(@Body() body: { email: string; password: string }) {
    const result = await this.auth.login(body.email, body.password);
    if (!result) return { error: 'Invalid credentials' };
    return result;
  }
}

@Controller('usage')
export class UsageController {
  constructor(private readonly rateLimit: RateLimitService) {}

  @Get('ngx-pulse')
  async getUsage() {
    return this.rateLimit.getUsageToday();
  }
}
