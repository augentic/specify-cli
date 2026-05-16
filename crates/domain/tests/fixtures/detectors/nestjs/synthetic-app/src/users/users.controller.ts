import { Controller, Get, Post, Delete } from '@nestjs/common';
import { UsersService } from './users.service';

@Controller('users')
export class UsersController {
  constructor(private usersService: UsersService) {}

  @Get()
  findAll() {
    return this.usersService.findAll();
  }

  @Post()
  create() {
    return this.usersService.create();
  }

  @Delete(':id')
  remove() {
    return this.usersService.remove();
  }
}
