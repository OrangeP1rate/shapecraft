//! B-spline basics from Piegl & Tiller, *The NURBS Book*: knot vectors, spans and basis functions. One copy, shared by
//! the spline fitter here and the Parasolid reader.

/// A knot vector written out in full: each distinct knot repeated by its multiplicity.
pub fn expand(knots: &[f64], mults: &[u32]) -> Vec<f64> {
    knots.iter().zip(mults).flat_map(|(k, m)| std::iter::repeat_n(*k, *m as usize)).collect()
}

/// The span of a full knot vector of degree `p` that holds `t` (A2.1): the last span takes the end of the range, and
/// a `t` outside it the end span on its side (carrying the curve on along that span's polynomial). `None` for a
/// vector with fewer than `p + 1` poles' worth of knots.
pub fn span(full: &[f64], p: usize, t: f64) -> Option<usize> {
    let poles = full.len().checked_sub(p + 1)?;
    if poles <= p {
        return None;
    }
    if t >= full[poles] {
        return Some(poles - 1);
    }
    if t <= full[p] {
        return Some(p);
    }
    let (mut lo, mut hi) = (p, poles);
    let mut mid = (lo + hi) / 2;
    while t < full[mid] || t >= full[mid + 1] {
        if t < full[mid] {
            hi = mid;
        } else {
            lo = mid;
        }
        mid = (lo + hi) / 2;
    }
    Some(mid)
}

/// The `p + 1` basis functions that don't vanish on `span` at `t`, and their first derivatives (A2.3 for k = 1).
pub fn basis_ders(full: &[f64], p: usize, span: usize, t: f64) -> (Vec<f64>, Vec<f64>) {
    let mut ndu = vec![vec![0.0; p + 1]; p + 1];
    let (mut left, mut right) = (vec![0.0; p + 1], vec![0.0; p + 1]);
    ndu[0][0] = 1.0;
    for j in 1..=p {
        left[j] = t - full[span + 1 - j];
        right[j] = full[span + j] - t;
        let mut saved = 0.0;
        for r in 0..j {
            ndu[j][r] = right[r + 1] + left[j - r];
            let tmp = if ndu[j][r] != 0.0 { ndu[r][j - 1] / ndu[j][r] } else { 0.0 };
            ndu[r][j] = saved + right[r + 1] * tmp;
            saved = left[j - r] * tmp;
        }
        ndu[j][j] = saved;
    }
    let n: Vec<f64> = (0..=p).map(|j| ndu[j][p]).collect();
    let d: Vec<f64> = if p == 0 {
        vec![0.0]
    } else {
        (0..=p)
            .map(|r| {
                // N'_r = p (N_{r,p-1} / (u_{i+p} - u_i) - N_{r+1,p-1} / (u_{i+p+1} - u_{i+1})), from the table.
                let a = if r >= 1 && ndu[p][r - 1] != 0.0 { ndu[r - 1][p - 1] / ndu[p][r - 1] } else { 0.0 };
                let b = if r < p && ndu[p][r] != 0.0 { ndu[r][p - 1] / ndu[p][r] } else { 0.0 };
                p as f64 * (a - b)
            })
            .collect()
    };
    (n, d)
}

/// The span holding `t` and the `p + 1` basis functions that don't vanish there (A2.1 and A2.2).
pub fn basis(full: &[f64], p: usize, t: f64) -> Option<(usize, Vec<f64>)> {
    let s = span(full, p, t)?;
    Some((s, basis_ders(full, p, s, t).0))
}

/// Every basis function at `t`, one per pole (zero away from `t`'s span).
pub fn basis_all(full: &[f64], p: usize, t: f64) -> Vec<f64> {
    let mut out = vec![0.0; full.len().saturating_sub(p + 1)];
    if let Some((s, n)) = basis(full, p, t) {
        for (k, v) in n.into_iter().enumerate() {
            if let Some(slot) = out.get_mut(s - p + k) {
                *slot = v;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    #[test]
    fn a_cubic_basis_sums_to_one_and_its_derivatives_to_zero() {
        let full = expand(&[0.0, 0.3, 0.7, 1.0], &[4, 1, 2, 4]);
        for t in [0.0, 0.1, 0.3, 0.5, 0.7, 0.99, 1.0] {
            let (s, n) = basis(&full, 3, t).unwrap();
            let (_, d) = basis_ders(&full, 3, s, t);
            assert!((n.iter().sum::<f64>() - 1.0).abs() < 1e-14, "{t}");
            assert!(d.iter().sum::<f64>().abs() < 1e-10, "{t}");
            assert_eq!(basis_all(&full, 3, t).len(), 7);
        }
        assert_eq!(span(&full, 3, 1.0), Some(6), "the end belongs to the last span");
        assert_eq!(span(&[0.0, 1.0], 3, 0.5), None);
    }
}
