// SPDX-License-Identifier: Apache-2.0
// Breakout 游戏状态机(M3): 纯软件模拟, 由平台层 30Hz 步进
// 玩法: 触摸 x → 挡板中心; 球碰壁反弹/碰挡板反弹/落底扣命; 24 砖清空重开

pub const BRICK_COLS: usize = 6;
pub const BRICK_ROWS: usize = 4;
pub const BRICK_COUNT: usize = BRICK_COLS * BRICK_ROWS;
/// 挡板 y(顶边), 官方 440
pub const PADDLE_Y: f32 = 440.0;
pub const PADDLE_W: f32 = 100.0;
pub const BALL_R: f32 = 5.0;
/// 砖区几何(与 app-breakout.slint 一致)
pub const BRICK_X0: f32 = 17.0;
pub const BRICK_Y0: f32 = 64.0;
pub const BRICK_W: f32 = 72.0;
pub const BRICK_H: f32 = 18.0;
pub const BRICK_DX: f32 = 78.0;
pub const BRICK_DY: f32 = 24.0;

pub struct Game {
    pub ball_x: f32,
    pub ball_y: f32,
    pub vx: f32,
    pub vy: f32,
    pub paddle_x: f32, // 挡板左边
    pub bricks: u32,
    pub score: u32,
    pub lives: u32,
    pub running: bool,
}

impl Game {
    pub fn new() -> Self {
        Self {
            ball_x: 240.0,
            ball_y: 300.0,
            vx: 2.4,
            vy: -2.8,
            paddle_x: 190.0,
            bricks: (1u32 << BRICK_COUNT) - 1,
            score: 0,
            lives: 3,
            running: false,
        }
    }

    /// 触摸 → 挡板中心目标(渲染坐标 0..480)
    pub fn set_paddle_target(&mut self, touch_x: f32) {
        self.paddle_x = (touch_x - PADDLE_W / 2.0).clamp(0.0, 480.0 - PADDLE_W);
    }

    pub fn start(&mut self) {
        if self.lives == 0 {
            *self = Self::new();
        }
        if !self.running {
            self.running = true;
        }
    }

    /// 一步物理(帧率无关: 调用方按 33ms 周期调用)
    pub fn step(&mut self) {
        if !self.running {
            return;
        }
        self.ball_x += self.vx;
        self.ball_y += self.vy;

        // 墙壁反弹
        if self.ball_x < BALL_R {
            self.ball_x = BALL_R;
            self.vx = -self.vx;
        }
        if self.ball_x > 480.0 - BALL_R {
            self.ball_x = 480.0 - BALL_R;
            self.vx = -self.vx;
        }
        if self.ball_y < 56.0 + BALL_R {
            self.ball_y = 56.0 + BALL_R;
            self.vy = -self.vy;
        }

        // 挡板碰撞(球底到达挡板顶且 x 在板内)
        if self.vy > 0.0
            && self.ball_y + BALL_R >= PADDLE_Y
            && self.ball_y + BALL_R <= PADDLE_Y + 14.0
            && self.ball_x >= self.paddle_x - BALL_R
            && self.ball_x <= self.paddle_x + PADDLE_W + BALL_R
        {
            self.ball_y = PADDLE_Y - BALL_R;
            // 反射角随击中位置偏移(中间垂直, 边缘斜)
            let hit = (self.ball_x - (self.paddle_x + PADDLE_W / 2.0)) / (PADDLE_W / 2.0);
            self.vx = hit * 3.2;
            self.vy = -libm::sqrtf(12.0 - self.vx * self.vx).max(1.2);
        }

        // 砖块碰撞(简化的 AABB)
        if self.vy < 0.0 || self.ball_y < BRICK_Y0 + BRICK_DY * BRICK_ROWS as f32 + 20.0 {
            for i in 0..BRICK_COUNT {
                if self.bricks & (1 << i) == 0 {
                    continue;
                }
                let bx = BRICK_X0 + (i % BRICK_COLS) as f32 * BRICK_DX;
                let by = BRICK_Y0 + (i / BRICK_COLS) as f32 * BRICK_DY;
                if self.ball_x + BALL_R >= bx
                    && self.ball_x - BALL_R <= bx + BRICK_W
                    && self.ball_y + BALL_R >= by
                    && self.ball_y - BALL_R <= by + BRICK_H
                {
                    self.bricks &= !(1 << i);
                    self.score += 10;
                    self.vy = -self.vy;
                    break;
                }
            }
        }

        // 落底扣命
        if self.ball_y > 480.0 + BALL_R * 2.0 {
            self.lives = self.lives.saturating_sub(1);
            self.running = self.lives > 0;
            self.reset_ball();
        }

        // 清空重开(加分)
        if self.bricks == 0 {
            self.bricks = (1u32 << BRICK_COUNT) - 1;
            self.score += 100;
            self.reset_ball();
        }
    }

    fn reset_ball(&mut self) {
        self.ball_x = 240.0;
        self.ball_y = 300.0;
        self.vx = 2.4;
        self.vy = -2.8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn game_over_starts_a_new_round() {
        let mut g = Game::new();
        g.lives = 0;
        g.score = 230;
        g.bricks = 0;
        g.start();
        assert!(g.running);
        assert_eq!(g.lives, 3);
        assert_eq!(g.score, 0);
        assert_eq!(g.bricks, (1 << BRICK_COUNT) - 1);
    }
    #[test]
    fn paused_game_does_not_move() {
        let mut g = Game::new();
        let p = (g.ball_x, g.ball_y);
        g.step();
        assert_eq!((g.ball_x, g.ball_y), p);
    }
    #[test]
    fn paddle_boundaries() {
        let mut g = Game::new();
        g.set_paddle_target(-1.0);
        assert_eq!(g.paddle_x, 0.0);
        g.set_paddle_target(600.0);
        assert_eq!(g.paddle_x, 380.0);
    }
}
