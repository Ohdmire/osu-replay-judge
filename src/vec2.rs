//! f32 vector math mirroring osuTK's `Vector2` float semantics.

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec2 {
    pub x: f32,
    pub y: f32,
}

impl Vec2 {
    pub const ZERO: Vec2 = Vec2 { x: 0.0, y: 0.0 };

    pub const fn new(x: f32, y: f32) -> Self {
        Vec2 { x, y }
    }

    #[inline]
    pub fn length_squared(self) -> f32 {
        self.x * self.x + self.y * self.y
    }

    #[inline]
    pub fn length(self) -> f32 {
        self.length_squared().sqrt()
    }

    #[inline]
    pub fn distance(a: Vec2, b: Vec2) -> f32 {
        (a - b).length()
    }

    #[inline]
    pub fn dot(a: Vec2, b: Vec2) -> f32 {
        a.x * b.x + a.y * b.y
    }

    #[inline]
    pub fn normalized(self) -> Vec2 {
        let len = self.length();
        if len == 0.0 {
            return Vec2::ZERO;
        }
        Vec2::new(self.x / len, self.y / len)
    }

    /// osuTK `Vector2.Lerp(a, b, blend)`: `a * (1 - blend) + b * blend`.
    pub fn lerp(a: Vec2, b: Vec2, blend: f64) -> Vec2 {
        let blend = blend as f32;
        Vec2::new(
            a.x * (1.0 - blend) + b.x * blend,
            a.y * (1.0 - blend) + b.y * blend,
        )
    }
}

impl std::ops::Add for Vec2 {
    type Output = Vec2;
    #[inline]
    fn add(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x + rhs.x, self.y + rhs.y)
    }
}

impl std::ops::AddAssign for Vec2 {
    #[inline]
    fn add_assign(&mut self, rhs: Vec2) {
        self.x += rhs.x;
        self.y += rhs.y;
    }
}

impl std::ops::Sub for Vec2 {
    type Output = Vec2;
    #[inline]
    fn sub(self, rhs: Vec2) -> Vec2 {
        Vec2::new(self.x - rhs.x, self.y - rhs.y)
    }
}

impl std::ops::Mul<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn mul(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x * rhs, self.y * rhs)
    }
}

impl std::ops::Div<f32> for Vec2 {
    type Output = Vec2;
    #[inline]
    fn div(self, rhs: f32) -> Vec2 {
        Vec2::new(self.x / rhs, self.y / rhs)
    }
}

impl std::ops::Neg for Vec2 {
    type Output = Vec2;
    #[inline]
    fn neg(self) -> Vec2 {
        Vec2::new(-self.x, -self.y)
    }
}

/// osu-framework `Precision` helpers (f32/f64 comparisons).
pub mod precision {
    /// Framework `Precision.FLOAT_EPSILON`.
    const FLOAT_EPSILON: f32 = 1e-3f32;
    /// Framework `Precision.DOUBLE_EPSILON`.
    const DOUBLE_EPSILON: f64 = 1e-7;

    pub fn almost_equals_f32(value1: f32, value2: f32) -> bool {
        (value1 - value2).abs() <= FLOAT_EPSILON
    }

    /// Framework `Precision.AlmostEquals(double,double)`.
    pub fn almost_equals_f64(value1: f64, value2: f64) -> bool {
        (value1 - value2).abs() <= DOUBLE_EPSILON
    }
}
