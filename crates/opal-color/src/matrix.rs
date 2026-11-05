//! 3×3 matrices, evaluated at compile time where possible.

/// A row-major 3×3 matrix.
pub type Mat3 = [[f64; 3]; 3];

/// `m · v`.
pub const fn mul_vec(m: &Mat3, v: [f64; 3]) -> [f64; 3] {
    [
        m[0][0] * v[0] + m[0][1] * v[1] + m[0][2] * v[2],
        m[1][0] * v[0] + m[1][1] * v[1] + m[1][2] * v[2],
        m[2][0] * v[0] + m[2][1] * v[1] + m[2][2] * v[2],
    ]
}

/// `a · b`.
pub const fn mul(a: &Mat3, b: &Mat3) -> Mat3 {
    let mut r = [[0.0; 3]; 3];
    let mut i = 0;
    while i < 3 {
        let mut j = 0;
        while j < 3 {
            r[i][j] = a[i][0] * b[0][j] + a[i][1] * b[1][j] + a[i][2] * b[2][j];
            j += 1;
        }
        i += 1;
    }
    r
}

/// The inverse of `m`, which must be invertible (all matrices passed here are color-space
/// transforms, whose determinants are far from zero).
pub const fn invert(m: &Mat3) -> Mat3 {
    let [a, b, c] = m[0];
    let [d, e, f] = m[1];
    let [g, h, i] = m[2];
    let det = a * (e * i - f * h) - b * (d * i - f * g) + c * (d * h - e * g);
    assert!(det.abs() > 1e-12, "singular color matrix");
    let inv = 1.0 / det;
    [
        [(e * i - f * h) * inv, (c * h - b * i) * inv, (b * f - c * e) * inv],
        [(f * g - d * i) * inv, (a * i - c * g) * inv, (c * d - a * f) * inv],
        [(d * h - e * g) * inv, (b * g - a * h) * inv, (a * e - b * d) * inv],
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_times_matrix_is_identity() {
        let m: Mat3 = [[0.41, 0.36, 0.18], [0.21, 0.72, 0.07], [0.02, 0.12, 0.95]];
        let p = mul(&invert(&m), &m);
        for (i, row) in p.iter().enumerate() {
            for (j, &v) in row.iter().enumerate() {
                let want = if i == j { 1.0 } else { 0.0 };
                assert!((v - want).abs() < 1e-12, "p[{i}][{j}] = {v}");
            }
        }
    }
}
