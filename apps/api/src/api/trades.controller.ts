import { Controller, Get, Logger, Query, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { DatabaseService } from '../database/database.service';
import { logStart } from '../common/log.util';

@Controller('trades')
@UseGuards(JwtAuthGuard)
export class TradesController {
  private readonly logger = new Logger(TradesController.name);

  constructor(private readonly db: DatabaseService) {}

  @Get()
  async list(@Query('page') page = '1', @Query('limit') limit = '20') {
    const log = logStart(this.logger, 'list', { page, limit });
    const offset = (Number(page) - 1) * Number(limit);
    const r = await this.db.query(
      `SELECT t.*, s.rationale, s.confidence FROM sandbox_trades t
       LEFT JOIN signals s ON s.id = t.signal_id
       ORDER BY t.executed_at DESC LIMIT $1 OFFSET $2`,
      [Number(limit), offset],
    );
    const count = await this.db.query('SELECT COUNT(*) FROM sandbox_trades');
    const result = { data: r.rows, total: Number(count.rows[0].count) };
    log.done({ total: result.total });
    return result;
  }
}
