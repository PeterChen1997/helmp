//! GPUI 标注预览，使用与导出一致的几何和马赛克色块。

use std::sync::Arc;

use gpui_kit::*;

use super::annotation::{Drawing, Mark, Shape};
use super::composite::MosaicSource;
use super::geometry::{self, Point as ScreenPoint, Rect};

fn point_at(p: ScreenPoint) -> Point<Pixels> {
    point(px(p.x as f32), px(p.y as f32))
}

fn bounds_at(rect: Rect) -> Bounds<Pixels> {
    Bounds::new(
        point_at(ScreenPoint::new(rect.x, rect.y)),
        size(px(rect.w as f32), px(rect.h as f32)),
    )
}

fn polyline(path: &mut PathBuilder, points: &[ScreenPoint]) {
    if let [first, rest @ ..] = points {
        path.move_to(point_at(*first));
        for point in rest {
            path.line_to(point_at(*point));
        }
    }
}

fn ellipse(path: &mut PathBuilder, r: Rect) {
    let radii = point(px((r.w / 2.) as f32), px((r.h / 2.) as f32));
    let left = point_at(ScreenPoint::new(r.x, r.y + r.h / 2.));
    let right = point_at(ScreenPoint::new(r.right(), r.y + r.h / 2.));
    path.move_to(left);
    path.arc_to(radii, px(0.), false, true, right);
    path.arc_to(radii, px(0.), false, true, left);
    path.close();
}

fn paint_path(window: &mut Window, path: PathBuilder, color: Hsla) {
    if let Ok(path) = path.build() {
        window.paint_path(path, color);
    }
}

fn paint_ink(window: &mut Window, shape: Shape<'_>, color: u32, width: f64, opacity: f64) {
    let color = Hsla::from(rgb(color)).opacity(opacity as f32);
    let mut path = PathBuilder::stroke(px(width as f32));
    match shape {
        Shape::Rectangle(r) => {
            polyline(
                &mut path,
                &[
                    ScreenPoint::new(r.x, r.y),
                    ScreenPoint::new(r.right(), r.y),
                    ScreenPoint::new(r.right(), r.bottom()),
                    ScreenPoint::new(r.x, r.bottom()),
                ],
            );
            path.close();
        }
        Shape::Ellipse(r) => ellipse(&mut path, r),
        Shape::Line(points) => polyline(&mut path, &points),
        Shape::Arrow { shaft, head } => {
            polyline(&mut path, &shaft);
            paint_path(window, path, color);
            path = PathBuilder::fill();
            polyline(&mut path, &head);
            path.close();
        }
        Shape::Freehand([p]) => {
            path = PathBuilder::fill();
            ellipse(
                &mut path,
                Rect::new(p.x - width / 2., p.y - width / 2., width, width),
            );
        }
        Shape::Freehand(points) => polyline(&mut path, points),
    }
    paint_path(window, path, color);
}

pub fn layer(
    selection: Rect,
    marks: Vec<Arc<Mark>>,
    mosaic: Option<Arc<MosaicSource>>,
) -> impl IntoElement {
    canvas(
        |_, _, _| {},
        move |_, (), window, _| {
            let mask = ContentMask {
                bounds: bounds_at(selection),
            };
            window.with_content_mask(Some(mask), |window| {
                for drawing in marks.iter().filter_map(|mark| mark.drawing()) {
                    match drawing {
                        Drawing::Ink(shape, color, width, opacity) => {
                            paint_ink(window, shape, color, width, opacity);
                        }
                        Drawing::Mosaic(area, block_size) => {
                            if let Some(source) = &mosaic {
                                if let Some(clip) =
                                    geometry::pixel_crop(selection, source.scale, source.pixels)
                                {
                                    source.tiles(area, block_size, clip, |rect, color| {
                                        let to_pt = |pixels| f64::from(pixels) / source.scale;
                                        let rect = Rect::new(
                                            to_pt(rect.x),
                                            to_pt(rect.y),
                                            to_pt(rect.w),
                                            to_pt(rect.h),
                                        );
                                        window.paint_quad(fill(bounds_at(rect), rgb(color)));
                                    });
                                }
                            } else {
                                window.paint_quad(fill(bounds_at(area), black()));
                            }
                        }
                    }
                }
            });
        },
    )
    .absolute()
    .size_full()
}
