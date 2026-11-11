//! 截图裁剪与标注合成。没有标注时直接裁 CGImage，保留原图的色彩空间和像素。

use std::path::Path;
use std::ptr::NonNull;
use std::sync::Arc;

use anyhow::{Context as _, Result};
use objc2::AnyThread as _;
use objc2::rc::autoreleasepool;
use objc2_app_kit::{NSBitmapImageFileType, NSBitmapImageRep};
use objc2_core_foundation::CFRetained;
use objc2_core_graphics::{
    CGBitmapContextCreate, CGBitmapContextCreateImage, CGColor, CGColorSpace, CGColorSpaceModel,
    CGContext, CGImage, CGImageAlphaInfo, CGInterpolationQuality, CGLineCap, CGLineJoin,
    kCGColorSpaceSRGB,
};
use objc2_foundation::{NSData, NSDictionary, NSPoint, NSRect, NSSize};

use super::annotation::{Drawing, Mark, Shape};
use super::geometry::{self, PixelRect, Point, Rect};
use crate::i18n::{tr, trf};

pub(super) fn load_image(path: &Path) -> Result<CFRetained<CGImage>> {
    let bytes =
        std::fs::read(path).with_context(|| trf!("读取 {}", "Couldn't read {}", path.display()))?;
    autoreleasepool(|_| {
        let image = NSBitmapImageRep::imageRepWithData(&NSData::with_bytes(&bytes))
            .and_then(|rep| rep.CGImage())
            .with_context(|| tr("截图无法解码", "Couldn't decode the screenshot"))?;
        // SAFETY: AppKit retains the image for the lifetime of `image`.
        Ok(unsafe { CFRetained::retain(NonNull::from(&*image)) })
    })
}

fn ns_rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
    NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
}

fn rgba(color: u32, opacity: f64) -> [f64; 4] {
    let channel = |shift: u32| f64::from((color >> shift) & 255) / 255.;
    [channel(16), channel(8), channel(0), opacity]
}

fn color_space() -> Result<CFRetained<CGColorSpace>> {
    // SAFETY: kCGColorSpaceSRGB is an immutable Core Graphics constant.
    CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB })).with_context(|| {
        tr(
            "无法读取 sRGB 色彩空间",
            "Couldn't load the sRGB color space",
        )
    })
}

pub struct MosaicSource {
    rgba: Vec<u8>,
    width: usize,
    height: usize,
    pub pixels: (u32, u32),
    pub scale: f64,
}

impl MosaicSource {
    pub(super) fn new(image: &CGImage, scale: f64) -> Result<Self> {
        let pixels = (
            CGImage::width(Some(image)) as u32,
            CGImage::height(Some(image)) as u32,
        );
        let width = (f64::from(pixels.0) / scale / 2.).ceil() as usize;
        let height = (f64::from(pixels.1) / scale / 2.).ceil() as usize;
        let mut rgba = vec![0; width * height * 4];
        let space = color_space()?;
        // SAFETY: rgba has one row of width * 4 bytes per row and outlives the context.
        let context = unsafe {
            CGBitmapContextCreate(
                rgba.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .with_context(|| tr("无法生成马赛克", "Couldn't generate the mosaic"))?;
        CGContext::set_interpolation_quality(Some(&context), CGInterpolationQuality::High);
        CGContext::draw_image(
            Some(&context),
            ns_rect(0., 0., width as f64, height as f64),
            Some(image),
        );
        drop(context);
        Ok(Self {
            rgba,
            width,
            height,
            pixels,
            scale,
        })
    }

    /// 预览和导出共用同一套色块。色块贴齐原图的像素边界，裁剪后也不会出现缝隙。
    pub fn tiles(
        &self,
        area: Rect,
        size: f64,
        clip: PixelRect,
        mut paint: impl FnMut(PixelRect, u32),
    ) {
        let Some(area) = geometry::pixel_crop(area, self.scale, self.pixels) else {
            return;
        };
        let block = (size * self.scale).round().max(1.) as u32;
        let right = area.x + area.w;
        let bottom = area.y + area.h;
        let clip_right = clip.x + clip.w;
        let clip_bottom = clip.y + clip.h;
        if clip.x >= right || clip.y >= bottom || clip_right <= area.x || clip_bottom <= area.y {
            return;
        }
        let start_x = area.x + clip.x.saturating_sub(area.x) / block * block;
        let start_y = area.y + clip.y.saturating_sub(area.y) / block * block;
        for y in (start_y..bottom.min(clip_bottom)).step_by(block as usize) {
            for x in (start_x..right.min(clip_right)).step_by(block as usize) {
                let w = block.min(right - x);
                let h = block.min(bottom - y);
                let visible_x = x.max(clip.x);
                let visible_y = y.max(clip.y);
                let rect = PixelRect {
                    x: visible_x,
                    y: visible_y,
                    w: (x + w).min(clip_right) - visible_x,
                    h: (y + h).min(clip_bottom) - visible_y,
                };
                paint(rect, self.block_color(x, y, w, h));
            }
        }
    }

    fn block_color(&self, x: u32, y: u32, w: u32, h: u32) -> u32 {
        let sample = |x, y| {
            let ix = x as usize * self.width / self.pixels.0 as usize;
            let iy = y as usize * self.height / self.pixels.1 as usize;
            let offset = (iy * self.width + ix) * 4;
            &self.rgba[offset..offset + 3]
        };
        let samples = [
            sample(x + w / 4, y + h / 4),
            sample(x + 3 * w / 4, y + h / 4),
            sample(x + w / 4, y + 3 * h / 4),
            sample(x + 3 * w / 4, y + 3 * h / 4),
        ];
        (0..3).fold(0, |color, channel| {
            let sum: u32 = samples.iter().map(|pixel| u32::from(pixel[channel])).sum();
            (color << 8) | (sum / 4)
        })
    }
}

pub fn crop_png(
    full: &CGImage,
    rect: PixelRect,
    scale: f64,
    marks: &[Arc<Mark>],
    mosaic: Option<&MosaicSource>,
) -> Result<Vec<u8>> {
    autoreleasepool(|_| {
        let area = ns_rect(rect.x.into(), rect.y.into(), rect.w.into(), rect.h.into());
        let cropped = CGImage::with_image_in_rect(Some(full), area)
            .with_context(|| tr("裁剪截图失败", "Couldn't crop the screenshot"))?;
        let output = if marks.is_empty() {
            cropped
        } else {
            // 导出从与预览相同的冻结图取样。取样失败时由调用方显示错误。
            let fallback;
            let mosaic = if mosaic.is_none()
                && marks
                    .iter()
                    .any(|mark| matches!(mark.drawing(), Some(Drawing::Mosaic(..))))
            {
                fallback = MosaicSource::new(full, scale)?;
                Some(&fallback)
            } else {
                mosaic
            };
            draw_marks(&cropped, rect, scale, marks, mosaic)?
        };
        let out = NSBitmapImageRep::initWithCGImage(NSBitmapImageRep::alloc(), &output);
        out.setSize(NSSize::new(
            f64::from(rect.w) / scale,
            f64::from(rect.h) / scale,
        ));
        // SAFETY: the PNG encoder accepts an empty properties dictionary.
        let png = unsafe {
            out.representationUsingType_properties(NSBitmapImageFileType::PNG, &NSDictionary::new())
        }
        .with_context(|| tr("PNG 编码失败", "Couldn't encode the image as PNG"))?;
        Ok(png.to_vec())
    })
}

fn draw_marks(
    image: &CGImage,
    crop: PixelRect,
    scale: f64,
    marks: &[Arc<Mark>],
    mosaic: Option<&MosaicSource>,
) -> Result<CFRetained<CGImage>> {
    let srgb = color_space()?;
    let original = CGImage::color_space(Some(image))
        .filter(|space| CGColorSpace::model(Some(space)) == CGColorSpaceModel::RGB);
    let (w, h) = (crop.w as usize, crop.h as usize);
    // SAFETY: passing a null buffer lets Core Graphics allocate and own the bitmap.
    let context = unsafe {
        CGBitmapContextCreate(
            std::ptr::null_mut(),
            w,
            h,
            8,
            0,
            Some(original.as_deref().unwrap_or(&srgb)),
            CGImageAlphaInfo::PremultipliedLast.0,
        )
    }
    .with_context(|| {
        tr(
            "无法合成截图标注",
            "Couldn't render the screenshot annotations",
        )
    })?;
    let ctx = Some(&*context);
    CGContext::set_interpolation_quality(ctx, CGInterpolationQuality::None);
    CGContext::draw_image(
        ctx,
        ns_rect(0., 0., crop.w.into(), crop.h.into()),
        Some(image),
    );

    // 标注坐标 y 向下。先翻转到左上角，再减去裁剪原点。
    CGContext::translate_ctm(ctx, 0., f64::from(crop.h));
    CGContext::scale_ctm(ctx, 1., -1.);
    for drawing in marks.iter().filter_map(|mark| mark.drawing()) {
        match drawing {
            Drawing::Ink(shape, color, width, opacity) => {
                CGContext::save_g_state(ctx);
                CGContext::translate_ctm(ctx, -f64::from(crop.x), -f64::from(crop.y));
                CGContext::scale_ctm(ctx, scale, scale);
                draw_ink(&context, &srgb, shape, color, width, opacity);
                CGContext::restore_g_state(ctx);
            }
            Drawing::Mosaic(area, size) => {
                let mosaic = mosaic.context("missing mosaic source")?;
                CGContext::set_should_antialias(ctx, false);
                mosaic.tiles(area, size, crop, |tile, color| {
                    set_color(&context, &srgb, color, 1.);
                    CGContext::fill_rect(
                        ctx,
                        ns_rect(
                            (tile.x - crop.x).into(),
                            (tile.y - crop.y).into(),
                            tile.w.into(),
                            tile.h.into(),
                        ),
                    );
                });
                CGContext::set_should_antialias(ctx, true);
            }
        }
    }
    CGBitmapContextCreateImage(ctx).with_context(|| {
        tr(
            "无法合成截图标注",
            "Couldn't render the screenshot annotations",
        )
    })
}

fn set_color(ctx: &CGContext, space: &CGColorSpace, color: u32, opacity: f64) {
    let rgba = rgba(color, opacity);
    // SAFETY: the sRGB space takes three color components and an alpha.
    let color = unsafe { CGColor::new(Some(space), rgba.as_ptr()) };
    CGContext::set_fill_color_with_color(Some(ctx), color.as_deref());
    CGContext::set_stroke_color_with_color(Some(ctx), color.as_deref());
}

fn draw_ink(
    ctx: &CGContext,
    space: &CGColorSpace,
    shape: Shape<'_>,
    color: u32,
    width: f64,
    opacity: f64,
) {
    set_color(ctx, space, color, opacity);
    let ctx = Some(ctx);
    CGContext::set_line_width(ctx, width);
    CGContext::set_line_cap(ctx, CGLineCap::Round);
    CGContext::set_line_join(ctx, CGLineJoin::Round);
    CGContext::begin_path(ctx);
    let path = |points: &[Point]| {
        let [first, rest @ ..] = points else { return };
        CGContext::move_to_point(ctx, first.x, first.y);
        for point in rest {
            CGContext::add_line_to_point(ctx, point.x, point.y);
        }
    };
    match shape {
        Shape::Rectangle(r) => CGContext::add_rect(ctx, ns_rect(r.x, r.y, r.w, r.h)),
        Shape::Ellipse(r) => CGContext::add_ellipse_in_rect(ctx, ns_rect(r.x, r.y, r.w, r.h)),
        Shape::Line(points) => path(&points),
        Shape::Arrow { shaft, head } => {
            path(&shaft);
            CGContext::stroke_path(ctx);
            path(&head);
            CGContext::close_path(ctx);
            CGContext::fill_path(ctx);
            return;
        }
        Shape::Freehand([p]) => {
            CGContext::fill_ellipse_in_rect(
                ctx,
                ns_rect(p.x - width / 2., p.y - width / 2., width, width),
            );
            return;
        }
        Shape::Freehand(points) => path(points),
    }
    CGContext::stroke_path(ctx);
}
