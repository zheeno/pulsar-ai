import { Processor, WorkerHost } from '@nestjs/bullmq';
import { Logger } from '@nestjs/common';
import { Job } from 'bullmq';
import { SignalGenerationService } from './signal-generation.service';
import { logStart } from '../common/log.util';

@Processor('signal-generation')
export class SignalGenerationProcessor extends WorkerHost {
  private readonly logger = new Logger(SignalGenerationProcessor.name);

  constructor(private readonly service: SignalGenerationService) {
    super();
  }

  async process(job: Job<{ portfolioId?: string }>): Promise<string[]> {
    const log = logStart(this.logger, 'process', { jobId: job.id, portfolioId: job.data.portfolioId });
    const signalIds = await this.service.generateForPortfolio(job.data.portfolioId);
    log.done({ signals: signalIds.length });
    return signalIds;
  }
}
