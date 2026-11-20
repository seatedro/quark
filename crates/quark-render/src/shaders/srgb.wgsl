// The sRGB transfer functions, per channel: encoded values to linear light
// and back.

fn srgb_decode(c: vec3<f32>) -> vec3<f32> {
    let lo = c / 12.92;
    let hi = pow((c + vec3<f32>(0.055)) / 1.055, vec3<f32>(2.4));
    return select(hi, lo, c <= vec3<f32>(0.04045));
}

fn srgb_encode(c: vec3<f32>) -> vec3<f32> {
    let lo = c * 12.92;
    let hi = 1.055 * pow(max(c, vec3<f32>(0.0)), vec3<f32>(1.0 / 2.4)) - vec3<f32>(0.055);
    return select(hi, lo, c <= vec3<f32>(0.0031308));
}

// One channel of `srgb_decode` and of `srgb_encode`.

fn srgb_to_linear(c: f32) -> f32 {
    return srgb_decode(vec3<f32>(c)).x;
}

fn linear_to_srgb(c: f32) -> f32 {
    return srgb_encode(vec3<f32>(c)).x;
}
