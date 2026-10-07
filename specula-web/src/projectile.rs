//! A projectile's flight, as `CCmpProjectileManager` moves it: a pure
//! function of its launch and the time since, so seeking needs no state.

use specula_assets::math::Mat4;
use specula_assets::math::Quat;
use specula_assets::math::Vec3;

/// `PROJECTILE_DECAY_TIME`: how long a projectile lies on the ground.
pub const DECAY_SECS: f32 = 30.0;
/// The engine checks for the ground once a frame; this is its frame.
const STOP_STEP_SECS: f32 = 1.0 / 60.0;
/// One that never meets the ground (off the map) is let go after this.
const MAX_FALL_SECS: f32 = 30.0;
/// `AdvanceProjectile` points the model along its last 0.1 s for big frame
/// steps, else along the frame step; this stands in for a short frame.
const HEADING_SECS: f32 = 0.01;

#[derive(Clone, Copy)]
pub struct Flight {
    origin: Vec3,
    v: Vec3,
    gravity: f32,
    time_hit: f32,
}

impl Flight {
    /// `LaunchProjectile`; `None` for a target straight below the launch,
    /// which the engine cannot fly either.
    pub fn new(origin: Vec3, target: Vec3, speed: f32, gravity: f32) -> Option<Self> {
        let offset = Vec3::new(
            target.x - origin.x,
            target.y - origin.y,
            target.z - origin.z,
        );
        let horizontal = offset.x.hypot(offset.z);
        if horizontal <= 0.0 || speed <= 0.0 {
            return None;
        }
        let time_hit = horizontal / speed;
        let mut v = offset * (1.0 / time_hit);
        v.y = offset.y / time_hit + 0.5 * gravity * time_hit;
        Some(Flight {
            origin,
            v,
            gravity,
            time_hit,
        })
    }

    pub fn time_hit(&self) -> f32 {
        self.time_hit
    }

    /// `Projectile::position`: past the target it slows down along the
    /// ground but keeps falling.
    pub fn position(&self, t: f32) -> Vec3 {
        let t2 = if t > self.time_hit {
            self.time_hit + (1.0 + t - self.time_hit).ln()
        } else {
            t
        };
        Vec3::new(
            self.origin.x + self.v.x * t2,
            self.origin.y + self.v.y * t2 - 0.5 * self.gravity * t * t,
            self.origin.z + self.v.z * t2,
        )
    }

    /// When `AdvanceProjectile` first finds it under `ground`, from the
    /// time it reaches the target on.
    pub fn stop_time(&self, ground: impl Fn(f32, f32) -> f32) -> f32 {
        let mut t = self.time_hit;
        while t < self.time_hit + MAX_FALL_SECS {
            let at = self.position(t);
            if at.y < ground(at.x, at.z) {
                return t;
            }
            t += STOP_STEP_SECS;
        }
        t
    }

    /// The model's transform at `t`: its +y along the motion, as
    /// `AdvanceProjectile` builds it. After `stop` it lies where it fell,
    /// at `ground` height.
    pub fn transform(&self, t: f32, stop: f32, ground: f32) -> Mat4 {
        let t = t.min(stop);
        let mut at = self.position(t);
        let before = self.position(t - HEADING_SECS);
        let delta = Vec3::new(at.x - before.x, at.y - before.y, at.z - before.z).normalized();
        let up = Vec3::new(0.0, 1.0, 0.0);
        let cross = Vec3::new(delta.z, 0.0, -delta.x);
        let axis = if cross.x * cross.x + cross.z * cross.z < 0.0001 {
            // Almost straight up or down: any axis across will do.
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            cross.normalized()
        };
        let angle = (up.x * delta.x + up.y * delta.y + up.z * delta.z)
            .clamp(-1.0, 1.0)
            .acos();
        let (sin, cos) = (angle / 2.0).sin_cos();
        let mut transform = Mat4::from_quat(Quat {
            x: axis.x * sin,
            y: axis.y * sin,
            z: axis.z * sin,
            w: cos,
        });
        if t >= stop {
            at.y = ground;
        }
        transform.translate(at);
        transform
    }
}
