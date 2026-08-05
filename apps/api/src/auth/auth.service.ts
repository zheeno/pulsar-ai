import { Injectable, Logger } from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';
import { DatabaseService } from '../database/database.service';
import * as crypto from 'crypto';
import { logStart } from '../common/log.util';

@Injectable()
export class AuthService {
  private readonly logger = new Logger(AuthService.name);

  constructor(
    private readonly db: DatabaseService,
    private readonly jwt: JwtService,
  ) {}

  async login(email: string, password: string): Promise<{ access_token: string } | null> {
    const log = logStart(this.logger, 'login', { email });
    const hash = crypto.createHash('sha256').update(password).digest('hex');
    const result = await this.db.query(
      'SELECT id, email FROM app_users WHERE email = $1 AND password_hash = $2',
      [email, hash],
    );
    if (result.rows.length === 0) {
      log.warn('invalid credentials');
      log.done({ success: false });
      return null;
    }
    const user = result.rows[0];
    const token = this.jwt.sign({ sub: user.id, email: user.email });
    log.done({ success: true, userId: user.id });
    return { access_token: token };
  }

  async validateUser(payload: { sub: string; email: string }) {
    const log = logStart(this.logger, 'validateUser', { userId: payload.sub });
    const user = { id: payload.sub, email: payload.email };
    log.done({ valid: true });
    return user;
  }
}
