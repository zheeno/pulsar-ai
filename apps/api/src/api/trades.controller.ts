import { Controller, Get, Query, UseGuards } from '@nestjs/common';
import { JwtAuthGuard } from '../auth/jwt-auth.guard';
import { DatabaseService } from '../database/database.service';

@Controller('trades')
@UseGuards(JwtAuthGuard)
export class TradesController {
  constructor(private readonly db: DatabaseService) {}

  @Get()
  async list(@Query('page') page = '1', @Query('limit') limit = '20') {
    const offset = (Number(page) - 1) * Number(limit);
    const r = await this.db.query(
      `SELECT t.*, s.rationale, s.confidence FROM sandbox_trades t
       LEFT JOIN signals s ON s.id = t.signal_id
       ORDER BY t.executed_at DESC LIMIT $1 OFFSET $2`,
      [Number(limit), offset],
    );
    const count = await this.db.query('SELECT COUNT(*) FROM sandbox_trades');
    return { data: r.rows, total: Number(count.rows[0].count) };
  }
}
