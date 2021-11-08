use super::{Coefficients, Convolution};
use crate::image_view::{TypedImageView, TypedImageViewMut};
use crate::pixels::U8;
use crate::CpuExtensions;

mod avx2;
mod native;

impl Convolution for U8 {
    fn horiz_convolution(
        src_image: TypedImageView<Self>,
        dst_image: TypedImageViewMut<Self>,
        offset: u32,
        coeffs: Coefficients,
        cpu_extensions: CpuExtensions,
    ) {
        match cpu_extensions {
            #[cfg(target_arch = "x86_64")]
            CpuExtensions::Avx2 => avx2::horiz_convolution(src_image, dst_image, offset, coeffs),
            _ => native::horiz_convolution(src_image, dst_image, offset, coeffs),
        }
    }

    fn vert_convolution(
        src_image: TypedImageView<Self>,
        dst_image: TypedImageViewMut<Self>,
        coeffs: Coefficients,
        cpu_extensions: CpuExtensions,
    ) {
        match cpu_extensions {
            #[cfg(target_arch = "x86_64")]
            CpuExtensions::Avx2 => avx2::vert_convolution(src_image, dst_image, coeffs),
            _ => native::vert_convolution(src_image, dst_image, coeffs),
        }
    }
}