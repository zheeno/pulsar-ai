import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { ExecutionService } from './execution.service';
import { logStart } from '../common/log.util';

@Processor('sandbox-execution')
export class ExecutionProcessor extends WorkerHost {
  private readonly logger = new Logger(ExecutionProcessor.name);

  constructor(private readonly service: ExecutionService) {
    super();
  }

  async process(job: Job<{ signalIds: string[] }>): Promise<number> {
    const log = logStart(this.logger, 'process', { jobId: job.id, signalCount: job.data.signalIds.length });
    const executed = await this.service.processSignals(job.data.signalIds);
    log.done({ executed });
    return executed;
  }
}
