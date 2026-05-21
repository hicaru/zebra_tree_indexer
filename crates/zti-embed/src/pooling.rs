pub enum PoolingStrategy {
    Mean,
    Cls,
}

/// Pool a single row from a contiguous `(seq * dim)` slice into a caller-owned
/// `dim`-length buffer. Mean pooling respects the attention mask via `valid`;
/// CLS takes index 0. Zero allocation on the hot path: the caller owns the
/// destination and we just write into it.
pub fn pool_row_into(strategy: &PoolingStrategy, data: &[f32], valid: usize, out: &mut [f32]) {
    let dim = out.len();
    match strategy {
        PoolingStrategy::Mean => {
            for v in out.iter_mut() {
                *v = 0.0;
            }
            if valid == 0 {
                return;
            }
            for j in 0..valid {
                let row = &data[j * dim..(j + 1) * dim];
                for (s, &v) in out.iter_mut().zip(row) {
                    *s += v;
                }
            }
            let c = (valid as f32).recip();
            for x in out.iter_mut() {
                *x *= c;
            }
        }
        PoolingStrategy::Cls => {
            out.copy_from_slice(&data[..dim]);
        }
    }
}
