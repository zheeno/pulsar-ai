import { Controller, Get, Logger, Query, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { DatabaseService } from '../database/database.service';
import { logStart } from '../common/log.util';

@Controller('signals')
@UseGuards(JwtAuthGuard)
export class SignalsController {
  private readonly logger = new Logger(SignalsController.name);

  constructor(private readonly db: DatabaseService) {}

  @Get()
  async list(
    @Query('symbol') symbol?: string,
    @Query('action') action?: string,
    @Query('page') page = '1',
    @Query('limit') limit = '20',
  ) {
    const log = logStart(this.logger, 'list', { symbol, action, page, limit });
    const offset = (Number(page) - 1) * Number(limit);
    const conditions: string[] = [];
    const params: unknown[] = [];
    let idx = 1;

    if (symbol) { conditions.push(`symbol = $${idx++}`); params.push(symbol); }
    if (action) { conditions.push(`action = $${idx++}`); params.push(action); }

    const where = conditions.length ? `WHERE ${conditions.join(' AND ')}` : '';
    params.push(Number(limit), offset);

    const r = await this.db.query(
      `SELECT * FROM signals ${where} ORDER BY generated_at DESC LIMIT $${idx++} OFFSET $${idx}`,
      params,
    );
    const count = await this.db.query(`SELECT COUNT(*) FROM signals ${where}`, params.slice(0, -2));
    const result = { data: r.rows, total: Number(count.rows[0].count), page: Number(page), limit: Number(limit) };
    log.done({ total: result.total });
    return result;
  }
}
