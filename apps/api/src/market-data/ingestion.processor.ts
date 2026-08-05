import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { IngestionService } from './ingestion.service';
import { logStart } from '../common/log.util';

@Processor('market-data-ingest')
export class IngestionProcessor extends WorkerHost {
  private readonly logger = new Logger(IngestionProcessor.name);

  constructor(private readonly ingestion: IngestionService) {
    super();
  }

  async process(job: Job<{ type: string }>): Promise<unknown> {
    const log = logStart(this.logger, 'process', { jobId: job.id, type: job.data.type });
    let result: unknown;
    switch (job.data.type) {
      case 'stocks':
        result = await this.ingestion.ingestStocks();
        break;
      case 'market':
        result = await this.ingestion.ingestMarket();
        break;
      case 'indices':
        result = await this.ingestion.ingestIndices();
        break;
      case 'backfill':
        result = await this.ingestion.backfillSymbols(5);
        break;
      default:
        log.warn('unknown job type');
        result = null;
    }
    log.done({ result });
    return result;
  }
}
