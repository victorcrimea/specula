// The y rotation of the model transform, as impostor.wgsl `turn`.
fn turn(v: vec3<f32>, sine: f32, cosine: f32) -> vec3<f32> {
    return vec3<f32>(v.x * cosine + v.z * sine, v.y, -v.x * sine + v.z * cosine);
}
