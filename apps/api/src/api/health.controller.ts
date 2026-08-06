import { Body, Controller, Get, Logger, Post } from '@nestjs/common';
import { AuthService } from '../auth/auth.service';
import { IngestionService } from '../market-data/ingestion.service';
import { SignalGenerationService } from '../signal-generation/signal-generation.service';
import { ExecutionService } from '../execution/execution.service';
import { DailySnapshotService } from '../orchestrator/daily-snapshot.service';
import { RateLimitService } from '../market-data/rate-limit.service';
import { NgxPulseClient } from '../market-data/ngx-pulse.client';
import { PortfolioService } from './portfolio.service';
import { logStart } from '../common/log.util';

@Controller()
export class HealthController {
  private readonly logger = new Logger(HealthController.name);

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
    const log = logStart(this.logger, 'runCycle');
    const forceIngest = { force: true };

    const ingested = await this.ingestion.ingestStocks(forceIngest);
    await this.ingestion.ingestMarket(forceIngest);
    await this.ingestion.ingestIndices(forceIngest);

    const signalIds = await this.signals.generateForPortfolio();
    const executed = await this.execution.processSignals(signalIds);

    try {
      await this.snapshot.createSnapshot();
    } catch (err) {
      this.logger.warn(`Daily snapshot failed: ${err instanceof Error ? err.message : err}`);
    }

    const portfolioId = await this.portfolio.getDefaultPortfolioId();
    const portfolio = portfolioId ? await this.portfolio.getPortfolio(portfolioId) : null;

    const warnings = ingested === 0
      ? ['Live market ingestion failed or was skipped — cycle continued using existing price data.']
      : [];

    const result = { ingested, signals: signalIds.length, executed, portfolio, warnings };
    log.done({ ingested, signals: signalIds.length, executed, warnings: warnings.length });
    return result;
  }
}

@Controller('auth')
export class AuthController {
  private readonly logger = new Logger(AuthController.name);

  constructor(private readonly auth: AuthService) {}

  @Post('login')
  async login(@Body() body: { email: string; password: string }) {
    const log = logStart(this.logger, 'login', { email: body.email });
    const result = await this.auth.login(body.email, body.password);
    if (!result) {
      log.warn('invalid credentials');
      log.done({ success: false });
      return { error: 'Invalid credentials' };
    }
    log.done({ success: true });
    return result;
  }
}

@Controller('usage')
export class UsageController {
  private readonly logger = new Logger(UsageController.name);

  constructor(
    private readonly rateLimit: RateLimitService,
    private readonly ngx: NgxPulseClient,
  ) {}

  @Get('ngx-pulse')
  async getUsage() {
    const log = logStart(this.logger, 'getUsage');
    const authMode = this.ngx.getAuthMode();
    const usage = await this.rateLimit.getUsageToday(authMode);
    log.done(usage);
    return usage;
  }
}
