import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { ExecutionService } from './execution.service';

@Processor('sandbox-execution')
export class ExecutionProcessor extends WorkerHost {
  private readonly logger = new Logger(ExecutionProcessor.name);

  constructor(private readonly service: ExecutionService) {
    super();
  }

  async process(job: Job<{ signalIds: string[] }>): Promise<number> {
    this.logger.log(`Processing ${job.data.signalIds.length} signals for execution`);
    return this.service.processSignals(job.data.signalIds);
  }
}
