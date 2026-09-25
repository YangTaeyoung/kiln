//! 데이터베이스 브랜드 로고(simple-icons, CC0)를 브랜드 색 타일 위에 흰색으로 그린다.

use crate::Driver;
use egui::{Color32, ColorImage, CornerRadius, Rect, Response, Sense, TextureHandle, TextureOptions, Ui, Vec2};

const POSTGRES: &str = include_str!("../assets/logos/postgresql.svg");
const MYSQL: &str = include_str!("../assets/logos/mysql.svg");
const MARIADB: &str = include_str!("../assets/logos/mariadb.svg");
const SQLITE: &str = include_str!("../assets/logos/sqlite.svg");

fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// 드라이버 브랜드 색.
pub fn brand_color(d: Driver) -> Color32 {
    match d {
        Driver::Postgres => hex(0x336791),
        Driver::MySql => hex(0x00758F),
        Driver::MariaDb => hex(0x003545),
        Driver::Sqlite => hex(0x0F80CC),
    }
}

fn svg(d: Driver) -> &'static str {
    match d {
        Driver::Postgres => POSTGRES,
        Driver::MySql => MYSQL,
        Driver::MariaDb => MARIADB,
        Driver::Sqlite => SQLITE,
    }
}

/// 흰색 글리프를 `px` 크기 정사각형으로 래스터화한다.
pub fn rasterize(d: Driver, px: u32) -> Option<ColorImage> {
    let px = px.clamp(8, 512);
    let src = svg(d).replacen("<svg ", "<svg fill=\"#ffffff\" ", 1);
    let tree = resvg::usvg::Tree::from_str(&src, &resvg::usvg::Options::default()).ok()?;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(px, px)?;
    let size = tree.size();
    let scale = px as f32 / size.width().max(size.height());
    let dx = (px as f32 - size.width() * scale) / 2.0;
    let dy = (px as f32 - size.height() * scale) / 2.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_row(scale, 0.0, 0.0, scale, dx, dy), &mut pixmap.as_mut());
    Some(ColorImage::from_rgba_premultiplied([px as usize, px as usize], pixmap.data()))
}

fn texture(ctx: &egui::Context, d: Driver, px: u32) -> Option<TextureHandle> {
    let id = egui::Id::new(("kiln-db-logo", d as u8, px));
    if let Some(t) = ctx.data(|m| m.get_temp::<TextureHandle>(id)) {
        return Some(t);
    }
    let img = rasterize(d, px)?;
    let tex = ctx.load_texture(format!("db-logo-{}-{px}", d as u8), img, TextureOptions::LINEAR);
    ctx.data_mut(|m| m.insert_temp(id, tex.clone()));
    Some(tex)
}

/// `rect` 에 브랜드 색 둥근 타일과 흰 로고를 그린다.
pub fn paint(ui: &Ui, rect: Rect, d: Driver) {
    let r = (rect.width().min(rect.height()) * 0.28).round().clamp(3.0, 12.0) as u8;
    ui.painter().rect_filled(rect, CornerRadius::same(r), brand_color(d));
    let glyph = rect.shrink(rect.width() * 0.2);
    let px = (glyph.width() * ui.ctx().pixels_per_point()).round() as u32;
    if let Some(tex) = texture(ui.ctx(), d, px) {
        ui.painter().image(tex.id(), glyph, Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), Color32::WHITE);
    }
}

/// 로고 타일 하나를 배치한다.
pub fn tile(ui: &mut Ui, size: f32, d: Driver) -> Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::splat(size), Sense::hover());
    paint(ui, rect, d);
    resp
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_driver_logo_rasterizes_with_visible_pixels() {
        for d in Driver::ALL {
            let img = rasterize(d, 48).expect("rasterize");
            let opaque = img.pixels.iter().filter(|p| p.a() > 128).count();
            assert!(opaque > 48 * 48 / 20, "{d:?} logo too empty: {opaque}");
        }
    }
}
