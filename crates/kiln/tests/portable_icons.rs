//! Render the shipped icon assets without any fonts installed on the host.
//! Captures contain only synthetic icons and stay in a temporary /tmp directory.

use egui::{Rect, pos2, vec2};
use egui_kittest::Harness;
use image::RgbaImage;
use kiln_common::Theme;
use kiln_common::icons::{self, Icon};

const ICON_SIZE: f32 = 20.0;
const SPACING: f32 = 40.0;
const MARGIN: f32 = 20.0;
const ICONS: [Icon; 19] = [
    Icon::Codex,
    Icon::Claude,
    Icon::GitHub,
    Icon::Terminal,
    Icon::Gear,
    Icon::Grid,
    Icon::SplitRight,
    Icon::SplitDown,
    Icon::More,
    Icon::Menu,
    Icon::Close,
    Icon::Bell,
    Icon::Warning,
    Icon::Search,
    Icon::Play,
    Icon::Stop,
    Icon::S3,
    Icon::Server,
    Icon::Tools,
];

fn crop(image: &RgbaImage, index: usize, row: usize, dpi: f32) -> RgbaImage {
    let x = ((MARGIN + index as f32 * SPACING) * dpi).round() as u32;
    let y = ((MARGIN + row as f32 * SPACING) * dpi).round() as u32;
    let size = (ICON_SIZE * dpi).round() as u32;
    image::imageops::crop_imm(image, x, y, size, size).to_image()
}

fn visible_mask(image: &RgbaImage, background: image::Rgba<u8>) -> Vec<bool> {
    image
        .pixels()
        .map(|pixel| (0..3).any(|channel| pixel[channel].abs_diff(background[channel]) >= 20))
        .collect()
}

#[test]
fn shipped_icons_remain_visible_and_distinct_without_system_fonts_at_all_dpis() {
    let captures = tempfile::Builder::new()
        .prefix("kiln-portable-icons-")
        .tempdir_in("/tmp")
        .unwrap()
        .keep();

    for theme in [Theme::KILN_DARK, Theme::KILN_LIGHT] {
        for dpi in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let mut initialized = false;
            let mut harness = Harness::builder()
                .with_size(vec2(MARGIN * 2.0 + ICONS.len() as f32 * SPACING, 100.0))
                .with_pixels_per_point(dpi)
                .wgpu()
                .build_ui(|ui| {
                    if !initialized {
                        // Crucially, this never discovers the developer's Nerd Font or
                        // native symbols. The rendered marks must work in a fresh install.
                        ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                        theme.apply(ui.ctx());
                        initialized = true;
                    }
                    let painter = ui.painter();
                    painter.rect_filled(ui.max_rect(), 0.0, theme.bg);
                    for row in 0..2 {
                        for order in 0..ICONS.len() {
                            let index = if row == 0 {
                                order
                            } else {
                                ICONS.len() - 1 - order
                            };
                            let rect = Rect::from_min_size(
                                pos2(
                                    MARGIN + index as f32 * SPACING,
                                    MARGIN + row as f32 * SPACING,
                                ),
                                vec2(ICON_SIZE, ICON_SIZE),
                            );
                            // A common tint makes brand identity depend on the mark,
                            // rather than passing because the colors happen to differ.
                            icons::paint(painter, rect, ICONS[index], theme.text);
                        }
                    }
                });
            harness.run_steps(3);
            let image = harness.render().expect("WGPU icon rendering failed");
            let capture = captures.join(format!("{}-{dpi}.png", theme.name));
            image.save(&capture).unwrap();
            let context = format!("{} at {dpi}×; capture {}", theme.name, capture.display());
            // Sample the explicit surface fill, inside egui's panel margin.
            let sample = (12.0 * dpi).round() as u32;
            let background = *image.get_pixel(sample, sample);
            let mut masks = Vec::new();

            for (index, icon) in ICONS.iter().enumerate() {
                let first = crop(&image, index, 0, dpi);
                let reversed = crop(&image, index, 1, dpi);
                // Translating vector geometry can round antialiasing by a few
                // channel levels; a changed outline or texture remains a failure.
                let max_delta = first
                    .as_raw()
                    .iter()
                    .zip(reversed.as_raw())
                    .map(|(a, b)| a.abs_diff(*b))
                    .max()
                    .unwrap();
                assert!(
                    max_delta <= 3,
                    "{icon:?} changes when painted in reverse order (delta {max_delta}): {context}"
                );
                let mask = visible_mask(&first, background);
                let visible = mask.iter().filter(|&&pixel| pixel).count();
                assert!(
                    visible >= mask.len() / 50,
                    "{icon:?} is blank or too faint ({visible} pixels): {context}"
                );
                if *icon == Icon::S3 {
                    // The official S3 asset deliberately has a green square
                    // background. Require its white bucket and original green
                    // instead of applying the monochrome silhouette condition.
                    let bucket = first
                        .pixels()
                        .filter(|p| p[0] >= 220 && p[1] >= 220 && p[2] >= 220)
                        .count();
                    let green = first
                        .pixels()
                        .filter(|p| {
                            p[0].abs_diff(122) <= 3
                                && p[1].abs_diff(161) <= 3
                                && p[2].abs_diff(22) <= 3
                        })
                        .count();
                    assert!(
                        bucket >= mask.len() / 50 && green > mask.len() / 2,
                        "S3 bucket or original brand colors are missing: {context}"
                    );
                } else {
                    assert!(
                        visible < mask.len() * 9 / 10,
                        "{icon:?} renders as a solid tile: {context}"
                    );
                }
                masks.push(mask);
            }

            // The three brands must also differ from the ordinary terminal, so
            // a missing mark silently replaced with >_ cannot pass this test.
            for first in 0..4 {
                for second in first + 1..4 {
                    let different = masks[first]
                        .iter()
                        .zip(&masks[second])
                        .filter(|(a, b)| a != b)
                        .count();
                    let smaller_mark = masks[first]
                        .iter()
                        .filter(|&&pixel| pixel)
                        .count()
                        .min(masks[second].iter().filter(|&&pixel| pixel).count());
                    assert!(
                        different >= (smaller_mark / 5).max(8),
                        "{:?} and {:?} are visually indistinguishable ({different} pixels): {context}",
                        ICONS[first],
                        ICONS[second],
                    );
                }
            }
            for (first, second) in [(16, 17), (16, 3), (17, 3)] {
                let different = masks[first]
                    .iter()
                    .zip(&masks[second])
                    .filter(|(a, b)| a != b)
                    .count();
                assert!(
                    different >= 8,
                    "remote provider marks are indistinguishable: {context}"
                );
            }
        }
    }
    eprintln!(
        "Synthetic portable icon review captures: {}",
        captures.display()
    );
}

#[test]
fn tools_wrench_actual_button_and_small_marks_remain_distinct() {
    use kiln_common::widgets::{self, ButtonKind};
    for theme in [Theme::KILN_DARK, Theme::KILN_LIGHT] {
        Theme::set_current(theme.name);
        let mut initialized = false;
        let mut h = Harness::builder()
            .with_size([320.0, 110.0])
            .wgpu()
            .build_ui(|ui| {
                if !initialized {
                    ui.ctx().set_fonts(kiln_common::fonts::definitions(false));
                    initialized = true;
                    return;
                }
                theme.apply(ui.ctx());
                ui.painter().rect_filled(ui.max_rect(), 0.0, theme.bg);
                ui.horizontal(|ui| {
                    widgets::button_with(ui, Some(Icon::Tools), "Tools", ButtonKind::Ghost, true);
                    widgets::button_with(ui, Some(Icon::Gear), "Settings", ButtonKind::Ghost, true);
                });
                for (row, size) in [14.0, 16.0].into_iter().enumerate() {
                    for (column, icon) in [Icon::Tools, Icon::Gear, Icon::Inspector]
                        .into_iter()
                        .enumerate()
                    {
                        icons::paint(
                            ui.painter(),
                            Rect::from_min_size(
                                pos2(20.0 + column as f32 * 40.0, 50.0 + row as f32 * 30.0),
                                vec2(size, size),
                            ),
                            icon,
                            theme.text,
                        );
                    }
                }
            });
        h.run_steps(3);
        let image = h.render().unwrap();
        for (row, size) in [14u32, 16].into_iter().enumerate() {
            let masks: Vec<_> = (0..3)
                .map(|column| {
                    let crop = image::imageops::crop_imm(
                        &image,
                        20 + column * 40,
                        50 + row as u32 * 30,
                        size,
                        size,
                    )
                    .to_image();
                    visible_mask(&crop, image.get_pixel(12, 45).to_owned())
                })
                .collect();
            for other in [1, 2] {
                assert!(
                    masks[0]
                        .iter()
                        .zip(&masks[other])
                        .filter(|(a, b)| a != b)
                        .count()
                        >= 8,
                    "wrench differs from settings and inspector at{size}pt"
                );
            }
        }
        image
            .save(format!("/tmp/kiln-tools-button-small-{}.png", theme.name))
            .unwrap();
    }
}
