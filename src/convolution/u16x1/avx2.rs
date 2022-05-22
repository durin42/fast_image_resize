use std::arch::x86_64::*;

use crate::convolution::{optimisations, Coefficients};
use crate::image_view::{FourRows, FourRowsMut, TypedImageView, TypedImageViewMut};
use crate::pixels::U16;
use crate::simd_utils;

#[inline]
pub(crate) fn horiz_convolution(
    src_image: TypedImageView<U16>,
    mut dst_image: TypedImageViewMut<U16>,
    offset: u32,
    coeffs: Coefficients,
) {
    let (values, window_size, bounds_per_pixel) =
        (coeffs.values, coeffs.window_size, coeffs.bounds);

    let normalizer_guard = optimisations::NormalizerGuard32::new(values);
    let coefficients_chunks = normalizer_guard.normalized_chunks(window_size, &bounds_per_pixel);
    let dst_height = dst_image.height().get();

    let src_iter = src_image.iter_4_rows(offset, dst_height + offset);
    let dst_iter = dst_image.iter_4_rows_mut();
    for (src_rows, dst_rows) in src_iter.zip(dst_iter) {
        unsafe {
            horiz_convolution_four_rows(
                src_rows,
                dst_rows,
                &coefficients_chunks,
                &normalizer_guard,
            );
        }
    }

    let mut yy = dst_height - dst_height % 4;
    while yy < dst_height {
        unsafe {
            horiz_convolution_one_row(
                src_image.get_row(yy + offset).unwrap(),
                dst_image.get_row_mut(yy).unwrap(),
                &coefficients_chunks,
                &normalizer_guard,
            );
        }
        yy += 1;
    }
}

/// For safety, it is necessary to ensure the following conditions:
/// - length of all rows in src_rows must be equal
/// - length of all rows in dst_rows must be equal
/// - coefficients_chunks.len() == dst_rows.0.len()
/// - max(chunk.start + chunk.values.len() for chunk in coefficients_chunks) <= src_row.0.len()
#[target_feature(enable = "avx2")]
unsafe fn horiz_convolution_four_rows(
    src_rows: FourRows<U16>,
    dst_rows: FourRowsMut<U16>,
    coefficients_chunks: &[optimisations::CoefficientsI32Chunk],
    normalizer_guard: &optimisations::NormalizerGuard32,
) {
    let (s_row0, s_row1, s_row2, s_row3) = src_rows;
    let s_rows = [s_row0, s_row1, s_row2, s_row3];
    let (d_row0, d_row1, d_row2, d_row3) = dst_rows;
    let d_rows = [d_row0, d_row1, d_row2, d_row3];
    let precision = normalizer_guard.precision();
    let half_error = 1i64 << (precision - 1);
    let mut ll_buf = [0i64; 4];

    /*
        |L0  | |L1  | |L2  | |L3  | |L4  | |L5  | |L6  | |L7  |
        |0001| |0203| |0405| |0607| |0809| |1011| |1213| |1415|

        Shuffle to extract L0 and L1 as i64:
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0

        Shuffle to extract L2 and L3 as i64:
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4

        Shuffle to extract L4 and L5 as i64:
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8

        Shuffle to extract L6 and L7 as i64:
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12
    */

    #[rustfmt::skip]
    let l0l1_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0,
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0,
    );
    #[rustfmt::skip]
    let l2l3_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4,
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4,
    );
    #[rustfmt::skip]
    let l4l5_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8,
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8,
    );
    #[rustfmt::skip]
    let l6l7_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12,
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12,
    );

    for (dst_x, coeffs_chunk) in coefficients_chunks.iter().enumerate() {
        let mut x: usize = coeffs_chunk.start as usize;
        let mut ll_sum = [_mm256_set1_epi64x(0); 4];

        let mut coeffs = coeffs_chunk.values;

        let coeffs_by_16 = coeffs.chunks_exact(16);
        coeffs = coeffs_by_16.remainder();

        for k in coeffs_by_16 {
            let coeff0189_i64x4 =
                _mm256_set_epi64x(k[9] as i64, k[8] as i64, k[1] as i64, k[0] as i64);
            let coeff23ab_i64x2 =
                _mm256_set_epi64x(k[11] as i64, k[10] as i64, k[3] as i64, k[2] as i64);
            let coeff45cd_i64x2 =
                _mm256_set_epi64x(k[13] as i64, k[12] as i64, k[5] as i64, k[4] as i64);
            let coeff67ef_i64x2 =
                _mm256_set_epi64x(k[15] as i64, k[14] as i64, k[7] as i64, k[6] as i64);

            for i in 0..4 {
                let mut sum = ll_sum[i];
                let source = simd_utils::loadu_si256(s_rows[i], x);

                let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l0l1_i64x4, coeff0189_i64x4));

                let l2l3_i64x4 = _mm256_shuffle_epi8(source, l2l3_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l2l3_i64x4, coeff23ab_i64x2));

                let l4l5_i64x4 = _mm256_shuffle_epi8(source, l4l5_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l4l5_i64x4, coeff45cd_i64x2));

                let l6l7_i64x4 = _mm256_shuffle_epi8(source, l6l7_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l6l7_i64x4, coeff67ef_i64x2));

                ll_sum[i] = sum;
            }
            x += 16;
        }

        let coeffs_by_8 = coeffs.chunks_exact(8);
        coeffs = coeffs_by_8.remainder();

        for k in coeffs_by_8 {
            let coeff0145_i64x4 =
                _mm256_set_epi64x(k[5] as i64, k[4] as i64, k[1] as i64, k[0] as i64);
            let coeff2367_i64x2 =
                _mm256_set_epi64x(k[7] as i64, k[6] as i64, k[3] as i64, k[2] as i64);

            for i in 0..4 {
                let mut sum = ll_sum[i];
                let source = _mm256_set_m128i(
                    simd_utils::loadl_epi64(s_rows[i], x + 4),
                    simd_utils::loadl_epi64(s_rows[i], x),
                );

                let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l0l1_i64x4, coeff0145_i64x4));

                let l2l3_i64x4 = _mm256_shuffle_epi8(source, l2l3_shuffle);
                sum = _mm256_add_epi64(sum, _mm256_mul_epi32(l2l3_i64x4, coeff2367_i64x2));

                ll_sum[i] = sum;
            }
            x += 8;
        }

        let coeffs_by_4 = coeffs.chunks_exact(4);
        coeffs = coeffs_by_4.remainder();

        for k in coeffs_by_4 {
            let coeff0123_i64x4 =
                _mm256_set_epi64x(k[3] as i64, k[2] as i64, k[1] as i64, k[0] as i64);

            for i in 0..4 {
                let source = _mm256_set_m128i(
                    simd_utils::loadl_epi32(s_rows[i], x + 2),
                    simd_utils::loadl_epi32(s_rows[i], x),
                );

                let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
                ll_sum[i] =
                    _mm256_add_epi64(ll_sum[i], _mm256_mul_epi32(l0l1_i64x4, coeff0123_i64x4));
            }
            x += 4;
        }

        if !coeffs.is_empty() {
            let mut coeffs_x3: [i64; 4] = [0; 4];
            for (d_coeff, &s_coeff) in coeffs_x3.iter_mut().zip(coeffs) {
                *d_coeff = s_coeff as i64;
            }
            let coeff0123_i64x4 = simd_utils::loadu_si256(&coeffs_x3, 0);

            for i in 0..4 {
                let mut pixels: [i64; 4] = [0; 4];
                let src_row = s_rows[i];
                for (i, pixel) in pixels.iter_mut().take(coeffs.len()).enumerate() {
                    *pixel = (*src_row.get_unchecked(x + i)).0 as i64;
                }
                let source = simd_utils::loadu_si256(&pixels, 0);
                ll_sum[i] = _mm256_add_epi64(ll_sum[i], _mm256_mul_epi32(source, coeff0123_i64x4));
            }
        }

        for i in 0..4 {
            _mm256_storeu_si256((&mut ll_buf).as_mut_ptr() as *mut __m256i, ll_sum[i]);
            let dst_pixel = d_rows[i].get_unchecked_mut(dst_x);
            dst_pixel.0 = normalizer_guard.clip(ll_buf.iter().sum::<i64>() + half_error);
        }
    }
}

/// For safety, it is necessary to ensure the following conditions:
/// - bounds.len() == dst_row.len()
/// - coefficients_chunks.len() == dst_row.len()
/// - max(chunk.start + chunk.values.len() for chunk in coefficients_chunks) <= src_row.len()
#[target_feature(enable = "avx2")]
unsafe fn horiz_convolution_one_row(
    src_row: &[U16],
    dst_row: &mut [U16],
    coefficients_chunks: &[optimisations::CoefficientsI32Chunk],
    normalizer_guard: &optimisations::NormalizerGuard32,
) {
    let precision = normalizer_guard.precision();
    let half_error = 1i64 << (precision - 1);
    let mut ll_buf = [0i64; 4];

    /*
        |L0  | |L1  | |L2  | |L3  | |L4  | |L5  | |L6  | |L7  |
        |0001| |0203| |0405| |0607| |0809| |1011| |1213| |1415|

        Shuffle to extract L0 and L1 as i64:
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0

        Shuffle to extract L2 and L3 as i64:
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4

        Shuffle to extract L4 and L5 as i64:
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8

        Shuffle to extract L6 and L7 as i64:
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12
    */

    #[rustfmt::skip]
    let l0l1_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0,
        -1, -1, -1, -1, -1, -1, 3, 2, -1, -1, -1, -1, -1, -1, 1, 0,
    );
    #[rustfmt::skip]
    let l2l3_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4,
        -1, -1, -1, -1, -1, -1, 7, 6, -1, -1, -1, -1, -1, -1, 5, 4,
    );
    #[rustfmt::skip]
    let l4l5_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8,
        -1, -1, -1, -1, -1, -1, 11, 10, -1, -1, -1, -1, -1, -1, 9, 8,
    );
    #[rustfmt::skip]
    let l6l7_shuffle = _mm256_set_epi8(
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12,
        -1, -1, -1, -1, -1, -1, 15, 14, -1, -1, -1, -1, -1, -1, 13, 12,
    );

    for (dst_x, coeffs_chunk) in coefficients_chunks.iter().enumerate() {
        let mut x: usize = coeffs_chunk.start as usize;
        let mut ll_sum = _mm256_set1_epi64x(0);
        let mut coeffs = coeffs_chunk.values;

        let coeffs_by_16 = coeffs.chunks_exact(16);
        coeffs = coeffs_by_16.remainder();

        for k in coeffs_by_16 {
            let coeff0189_i64x4 =
                _mm256_set_epi64x(k[9] as i64, k[8] as i64, k[1] as i64, k[0] as i64);
            let coeff23ab_i64x2 =
                _mm256_set_epi64x(k[11] as i64, k[10] as i64, k[3] as i64, k[2] as i64);
            let coeff45cd_i64x2 =
                _mm256_set_epi64x(k[13] as i64, k[12] as i64, k[5] as i64, k[4] as i64);
            let coeff67ef_i64x2 =
                _mm256_set_epi64x(k[15] as i64, k[14] as i64, k[7] as i64, k[6] as i64);

            let source = simd_utils::loadu_si256(src_row, x);

            let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l0l1_i64x4, coeff0189_i64x4));

            let l2l3_i64x4 = _mm256_shuffle_epi8(source, l2l3_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l2l3_i64x4, coeff23ab_i64x2));

            let l4l5_i64x4 = _mm256_shuffle_epi8(source, l4l5_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l4l5_i64x4, coeff45cd_i64x2));

            let l6l7_i64x4 = _mm256_shuffle_epi8(source, l6l7_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l6l7_i64x4, coeff67ef_i64x2));

            x += 16;
        }

        let coeffs_by_8 = coeffs.chunks_exact(8);
        coeffs = coeffs_by_8.remainder();

        for k in coeffs_by_8 {
            let coeff0145_i64x4 =
                _mm256_set_epi64x(k[5] as i64, k[4] as i64, k[1] as i64, k[0] as i64);
            let coeff2367_i64x2 =
                _mm256_set_epi64x(k[7] as i64, k[6] as i64, k[3] as i64, k[2] as i64);

            let source = _mm256_set_m128i(
                simd_utils::loadl_epi64(src_row, x + 4),
                simd_utils::loadl_epi64(src_row, x),
            );

            let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l0l1_i64x4, coeff0145_i64x4));

            let l2l3_i64x4 = _mm256_shuffle_epi8(source, l2l3_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l2l3_i64x4, coeff2367_i64x2));

            x += 8;
        }

        let coeffs_by_4 = coeffs.chunks_exact(4);
        coeffs = coeffs_by_4.remainder();

        for k in coeffs_by_4 {
            let coeff0123_i64x4 =
                _mm256_set_epi64x(k[3] as i64, k[2] as i64, k[1] as i64, k[0] as i64);

            let source = _mm256_set_m128i(
                simd_utils::loadl_epi32(src_row, x + 2),
                simd_utils::loadl_epi32(src_row, x),
            );

            let l0l1_i64x4 = _mm256_shuffle_epi8(source, l0l1_shuffle);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(l0l1_i64x4, coeff0123_i64x4));

            x += 4;
        }

        if !coeffs.is_empty() {
            let mut coeffs_x4: [i64; 4] = [0; 4];
            for (d_coeff, &s_coeff) in coeffs_x4.iter_mut().zip(coeffs) {
                *d_coeff = s_coeff as i64;
            }
            let coeff0123_i64x4 = simd_utils::loadu_si256(&coeffs_x4, 0);

            let mut pixels: [i64; 4] = [0; 4];
            for pixel in pixels.iter_mut().take(coeffs.len()) {
                *pixel = (*src_row.get_unchecked(x)).0 as i64;
                x += 1;
            }
            let source = simd_utils::loadu_si256(&pixels, 0);
            ll_sum = _mm256_add_epi64(ll_sum, _mm256_mul_epi32(source, coeff0123_i64x4));
        }

        _mm256_storeu_si256((&mut ll_buf).as_mut_ptr() as *mut __m256i, ll_sum);
        let dst_pixel = dst_row.get_unchecked_mut(dst_x);
        dst_pixel.0 = normalizer_guard.clip(ll_buf.iter().sum::<i64>() + half_error);
    }
}