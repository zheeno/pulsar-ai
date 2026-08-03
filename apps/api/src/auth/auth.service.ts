import { Injectable } from '@nestjs/common';
import { JwtService } from '@nestjs/jwt';
import { DatabaseService } from '../database/database.service';
import * as crypto from 'crypto';

@Injectable()
export class AuthService {
  constructor(
    private readonly db: DatabaseService,
    private readonly jwt: JwtService,
  ) {}

  async login(email: string, password: string): Promise<{ access_token: string } | null> {
    const hash = crypto.createHash('sha256').update(password).digest('hex');
    const result = await this.db.query(
      'SELECT id, email FROM app_users WHERE email = $1 AND password_hash = $2',
      [email, hash],
    );
    if (result.rows.length === 0) return null;
    const user = result.rows[0];
    const token = this.jwt.sign({ sub: user.id, email: user.email });
    return { access_token: token };
  }

  async validateUser(payload: { sub: string; email: string }) {
    return { id: payload.sub, email: payload.email };
  }
}
