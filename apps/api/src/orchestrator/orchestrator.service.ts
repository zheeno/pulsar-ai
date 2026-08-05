import { Injectable, Logger, OnModuleInit } from '@nestjs/common';
import { InjectQueue } from '@nestjs/bullmq';
import { Queue } from 'bullmq';
import { Cron } from '@nestjs/schedule';
import { MARKET_DATA_QUEUE } from '../market-data/market-data.module';
import { DailySnapshotService } from './daily-snapshot.service';
import { TradingCalendarService } from '../market-data/trading-calendar.service';
import { logStart } from '../common/log.util';

@Injectable()
export class OrchestratorService implements OnModuleInit {
  private readonly logger = new Logger(OrchestratorService.name);

  constructor(
    @InjectQueue(MARKET_DATA_QUEUE) private readonly ingestQueue: Queue,
    private readonly snapshotService: DailySnapshotService,
    private readonly calendar: TradingCalendarService,
  ) {}

  onModuleInit() {
    this.logger.log('Orchestrator initialized');
  }

  @Cron('0 */30 9-15 * * 1-5', { timeZone: 'Africa/Lagos' })
  async scheduledIngestStocks() {
    const log = logStart(this.logger, 'scheduledIngestStocks');
    if (!this.calendar.isMarketOpen()) {
      log.debug('skipped', { reason: 'market closed' });
      log.done();
      return;
    }
    await this.ingestQueue.add('stocks', { type: 'stocks' });
    log.done({ queued: 'stocks' });
  }

  @Cron('0 0 10-15 * * 1-5', { timeZone: 'Africa/Lagos' })
  async scheduledIngestMarket() {
    const log = logStart(this.logger, 'scheduledIngestMarket');
    if (!this.calendar.isTradingDay()) {
      log.debug('skipped', { reason: 'not a trading day' });
      log.done();
      return;
    }
    await this.ingestQueue.add('market', { type: 'market' });
    await this.ingestQueue.add('indices', { type: 'indices' });
    log.done({ queued: ['market', 'indices'] });
  }

  @Cron('5 16 * * 1-5', { timeZone: 'Africa/Lagos' })
  async postCloseReconciliation() {
    const log = logStart(this.logger, 'postCloseReconciliation');
    await this.ingestQueue.add('stocks', { type: 'stocks' });
    await this.snapshotService.createSnapshot();
    log.done();
  }

  @Cron('0 10 * * 0,6', { timeZone: 'Africa/Lagos' })
  async weekendBackfill() {
    const log = logStart(this.logger, 'weekendBackfill');
    await this.ingestQueue.add('backfill', { type: 'backfill' });
    log.done({ queued: 'backfill' });
  }
}
