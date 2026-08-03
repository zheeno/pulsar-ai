import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { IngestionService } from './ingestion.service';

@Processor('market-data-ingest')
export class IngestionProcessor extends WorkerHost {
  private readonly logger = new Logger(IngestionProcessor.name);

  constructor(private readonly ingestion: IngestionService) {
    super();
  }

  async process(job: Job<{ type: string }>): Promise<unknown> {
    this.logger.log(`Processing job ${job.name}: ${job.data.type}`);
    switch (job.data.type) {
      case 'stocks':
        return this.ingestion.ingestStocks();
      case 'market':
        return this.ingestion.ingestMarket();
      case 'indices':
        return this.ingestion.ingestIndices();
      case 'backfill':
        return this.ingestion.backfillSymbols(5);
      default:
        return null;
    }
  }
}
