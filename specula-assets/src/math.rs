//! The few pieces of the engine's maths the skinning needs, ported from
//! `maths/Matrix3D` and `maths/Quaternion` so the multiplication order is
//! the engine's own. The bakes and the web client share them, so both
//! place bones and props the same way.

use std::ops::Add;
use std::ops::Mul;
use std::ops::Neg;

#[derive(Clone, Copy, Default, Debug, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    pub fn new(x: f32, y: f32, z: f32) -> Self {
        Vec3 { x, y, z }
    }

    pub fn length(self) -> f32 {
        (self.x * self.x + self.y * self.y + self.z * self.z).sqrt()
    }

    pub fn normalized(self) -> Self {
        let length = self.length();
        if length > 0.0 {
            self * (1.0 / length)
        } else {
            self
        }
    }

    pub fn min(self, other: Vec3) -> Self {
        Vec3::new(
            self.x.min(other.x),
            self.y.min(other.y),
            self.z.min(other.z),
        )
    }

    pub fn max(self, other: Vec3) -> Self {
        Vec3::new(
            self.x.max(other.x),
            self.y.max(other.y),
            self.z.max(other.z),
        )
    }

    /// `Interpolate` in maths/MathUtil.h.
    pub fn lerp(self, to: Vec3, ratio: f32) -> Vec3 {
        self + (to + -self) * ratio
    }
}

impl Add for Vec3 {
    type Output = Vec3;

    fn add(self, other: Vec3) -> Vec3 {
        Vec3::new(self.x + other.x, self.y + other.y, self.z + other.z)
    }
}

impl Mul<f32> for Vec3 {
    type Output = Vec3;

    fn mul(self, factor: f32) -> Vec3 {
        Vec3::new(self.x * factor, self.y * factor, self.z * factor)
    }
}

impl Neg for Vec3 {
    type Output = Vec3;

    fn neg(self) -> Vec3 {
        Vec3::new(-self.x, -self.y, -self.z)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Quat {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub w: f32,
}

impl Quat {
    pub const IDENTITY: Quat = Quat {
        x: 0.0,
        y: 0.0,
        z: 0.0,
        w: 1.0,
    };

    /// The Hamilton product: rotating by the result rotates by `other`
    /// first, matching `from_quat(self).mul(&from_quat(other))`.
    pub fn product(self, other: Quat) -> Quat {
        Quat {
            x: self.w * other.x + self.x * other.w + self.y * other.z - self.z * other.y,
            y: self.w * other.y - self.x * other.z + self.y * other.w + self.z * other.x,
            z: self.w * other.z + self.x * other.y - self.y * other.x + self.z * other.w,
            w: self.w * other.w - self.x * other.x - self.y * other.y - self.z * other.z,
        }
    }

    pub fn rotate(self, v: Vec3) -> Vec3 {
        Mat4::from_quat(self).rotate(v)
    }

    /// `CQuaternion::Slerp`, which `CSkeletonAnimDef::BuildBoneMatrices`
    /// uses between keys.
    pub fn slerp(self, to: Quat, ratio: f32) -> Quat {
        const EPSILON: f32 = 0.0001;
        let mut cosom = self.x * to.x + self.y * to.y + self.z * to.z + self.w * to.w;
        let to = if cosom < 0.0 {
            cosom = -cosom;
            Quat {
                x: -to.x,
                y: -to.y,
                z: -to.z,
                w: -to.w,
            }
        } else {
            to
        };
        let (scale0, scale1) = if 1.0 - cosom > EPSILON {
            let omega = cosom.acos();
            let sinom = omega.sin();
            (
                ((1.0 - ratio) * omega).sin() / sinom,
                (ratio * omega).sin() / sinom,
            )
        } else {
            (1.0 - ratio, ratio)
        };
        Quat {
            x: scale0 * self.x + scale1 * to.x,
            y: scale0 * self.y + scale1 * to.y,
            z: scale0 * self.z + scale1 * to.z,
            w: scale0 * self.w + scale1 * to.w,
        }
    }

    /// `CQuaternion::GetInverse`: the conjugate, since rotations are unit
    /// length.
    pub fn inverse(self) -> Self {
        Quat {
            x: -self.x,
            y: -self.y,
            z: -self.z,
            w: self.w,
        }
    }
}

/// `m[row][column]` for column vectors, like CMatrix3D's `_rc` names.
#[derive(Clone, Copy, Debug)]
pub struct Mat4 {
    pub m: [[f32; 4]; 4],
}

impl Mat4 {
    pub fn identity() -> Self {
        let mut m = [[0.0; 4]; 4];
        for (i, row) in m.iter_mut().enumerate() {
            row[i] = 1.0;
        }
        Mat4 { m }
    }

    /// `CQuaternion::ToMatrix`.
    pub fn from_quat(q: Quat) -> Self {
        let xx = q.x * q.x * 2.0;
        let xy = q.x * q.y * 2.0;
        let xz = q.x * q.z * 2.0;
        let yy = q.y * q.y * 2.0;
        let yz = q.y * q.z * 2.0;
        let zz = q.z * q.z * 2.0;
        let wx = q.w * q.x * 2.0;
        let wy = q.w * q.y * 2.0;
        let wz = q.w * q.z * 2.0;
        Mat4 {
            m: [
                [1.0 - (yy + zz), xy - wz, xz + wy, 0.0],
                [xy + wz, 1.0 - (xx + zz), yz - wx, 0.0],
                [xz - wy, yz + wx, 1.0 - (xx + yy), 0.0],
                [0.0, 0.0, 0.0, 1.0],
            ],
        }
    }

    /// `CMatrix3D::Translate`: the translation applies after this matrix.
    pub fn translate(&mut self, v: Vec3) {
        self.m[0][3] += v.x;
        self.m[1][3] += v.y;
        self.m[2][3] += v.z;
    }

    /// `CMatrix3D::Rotate(CQuaternion)`: the rotation applies after this
    /// matrix.
    pub fn rotate_by(&mut self, q: Quat) {
        self.concatenate(&Mat4::from_quat(q));
    }

    /// `CMatrix3D::Concatenate`: `self = other * self`.
    pub fn concatenate(&mut self, other: &Mat4) {
        *self = other.mul(self);
    }

    pub fn mul(&self, other: &Mat4) -> Mat4 {
        let mut m = [[0.0; 4]; 4];
        for (r, row) in m.iter_mut().enumerate() {
            for (c, value) in row.iter_mut().enumerate() {
                *value = (0..4).map(|k| self.m[r][k] * other.m[k][c]).sum();
            }
        }
        Mat4 { m }
    }

    pub fn transform(&self, v: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z + m[0][3],
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z + m[1][3],
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z + m[2][3],
        )
    }

    pub fn rotate(&self, v: Vec3) -> Vec3 {
        let m = &self.m;
        Vec3::new(
            m[0][0] * v.x + m[0][1] * v.y + m[0][2] * v.z,
            m[1][0] * v.x + m[1][1] * v.y + m[1][2] * v.z,
            m[2][0] * v.x + m[2][1] * v.y + m[2][2] * v.z,
        )
    }

    /// A bone or key state as the engine builds it: rotate, then translate.
    pub fn from_bone(translation: Vec3, rotation: Quat) -> Self {
        let mut matrix = Mat4::from_quat(rotation);
        matrix.translate(translation);
        matrix
    }

    /// The inverse of `from_bone`, built as `CModelDef::Load` builds the
    /// inverse bind matrices.
    pub fn inverse_bone(translation: Vec3, rotation: Quat) -> Self {
        let mut matrix = Mat4::identity();
        matrix.translate(-translation);
        matrix.rotate_by(rotation.inverse());
        matrix
    }
}
