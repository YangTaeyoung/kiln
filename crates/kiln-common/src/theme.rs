//! 디자인 토큰(색·모서리·간격)과 테마. 모든 크레이트가 `Theme::current()` 로 읽는다.

use egui::{Color32, CornerRadius, FontFamily, FontId, Margin, Shadow, Stroke, TextStyle, Vec2};
use std::sync::atomic::{AtomicUsize, Ordering};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Theme {
    pub name: &'static str,
    pub label: &'static str,
    pub dark: bool,
    /// 창 바탕(터미널·에디터 영역).
    pub bg: Color32,
    /// 사이드바·패널.
    pub bg_panel: Color32,
    /// 팝업·카드·입력칸.
    pub bg_elevated: Color32,
    pub bg_hover: Color32,
    pub bg_selected: Color32,
    pub bg_input: Color32,
    pub border: Color32,
    pub border_strong: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    /// 강조색 위 글자.
    pub accent_fg: Color32,
    pub green: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub purple: Color32,
    pub orange: Color32,
    /// 터미널 ANSI 16색.
    pub ansi: [Color32; 16],
}

const fn hex(v: u32) -> Color32 {
    Color32::from_rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

pub const RADIUS_SM: u8 = 5;
pub const RADIUS: u8 = 8;
pub const RADIUS_LG: u8 = 12;

impl Theme {
    pub const KILN_DARK: Theme = Theme {
        name: "kiln-dark",
        label: "Kiln 다크",
        dark: true,
        bg: hex(0x14171c),
        bg_panel: hex(0x1a1e25),
        bg_elevated: hex(0x232832),
        bg_hover: hex(0x2b323e),
        bg_selected: hex(0x303c50),
        bg_input: hex(0x161b22),
        border: hex(0x2b323e),
        border_strong: hex(0x3b4656),
        text: hex(0xecf0f7),
        text_dim: hex(0xb2bdcf),
        text_faint: hex(0xa1a6b3),
        accent: hex(0x91b4ff),
        accent_fg: hex(0x101726),
        green: hex(0x5fd08c),
        red: hex(0xff8383),
        yellow: hex(0xf2c46d),
        blue: hex(0x5eb1ff),
        purple: hex(0xc49bff),
        orange: hex(0xff9f5a),
        ansi: [
            hex(0x2b2d33), hex(0xff6b6b), hex(0x5fd08c), hex(0xf2c46d), hex(0x5eb1ff), hex(0xc49bff), hex(0x5ad4d4), hex(0xc8cad2),
            hex(0x5c5f6a), hex(0xff8a8a), hex(0x80e0a6), hex(0xffd88c), hex(0x85c5ff), hex(0xd6b8ff), hex(0x7ee6e6), hex(0xf4f5f8),
        ],
    };

    pub const MIDNIGHT: Theme = Theme {
        name: "midnight",
        label: "미드나잇",
        dark: true,
        bg: hex(0x0d1117),
        bg_panel: hex(0x121821),
        bg_elevated: hex(0x19212c),
        bg_hover: hex(0x1f2936),
        bg_selected: hex(0x243247),
        bg_input: hex(0x141b25),
        border: hex(0x1f2936),
        border_strong: hex(0x2d3a4c),
        text: hex(0xe6edf3),
        text_dim: hex(0x9aa7b8),
        text_faint: hex(0x929fb2),
        accent: hex(0x4f9dff),
        accent_fg: hex(0x0c1524),
        green: hex(0x56d364),
        red: hex(0xf8716d),
        yellow: hex(0xe3b341),
        blue: hex(0x79c0ff),
        purple: hex(0xd2a8ff),
        orange: hex(0xffa657),
        ansi: [
            hex(0x21262d), hex(0xf8716d), hex(0x56d364), hex(0xe3b341), hex(0x58a6ff), hex(0xbc8cff), hex(0x39c5cf), hex(0xb1bac4),
            hex(0x6e7681), hex(0xffa198), hex(0x7ee787), hex(0xf2cc60), hex(0x79c0ff), hex(0xd2a8ff), hex(0x56d4dd), hex(0xf0f6fc),
        ],
    };

    pub const EMBER: Theme = Theme {
        name: "ember",
        label: "엠버",
        dark: true,
        bg: hex(0x141210),
        bg_panel: hex(0x1a1714),
        bg_elevated: hex(0x231f1b),
        bg_hover: hex(0x2b2621),
        bg_selected: hex(0x362e26),
        bg_input: hex(0x1d1a16),
        border: hex(0x2b2621),
        border_strong: hex(0x3b332b),
        text: hex(0xf1ebe4),
        text_dim: hex(0xb3a898),
        text_faint: hex(0xa99c8c),
        accent: hex(0xff8a3d),
        accent_fg: hex(0x1a1208),
        green: hex(0x9ccf74),
        red: hex(0xf86e65),
        yellow: hex(0xf0c05a),
        blue: hex(0x78b4e8),
        purple: hex(0xd09ade),
        orange: hex(0xff8a3d),
        ansi: [
            hex(0x2e2924), hex(0xf2685f), hex(0x9ccf74), hex(0xf0c05a), hex(0x78b4e8), hex(0xd09ade), hex(0x75c9b7), hex(0xd6cdc2),
            hex(0x6b6157), hex(0xff8b82), hex(0xb7e395), hex(0xffd47e), hex(0x9ccbf2), hex(0xe2b8ec), hex(0x97dccd), hex(0xfaf6f1),
        ],
    };

    pub const KILN_LIGHT: Theme = Theme {
        name: "kiln-light",
        label: "Kiln 라이트",
        dark: false,
        bg: hex(0xffffff),
        bg_panel: hex(0xf6f6f8),
        bg_elevated: hex(0xffffff),
        bg_hover: hex(0xecedf1),
        bg_selected: hex(0xe3e6f7),
        bg_input: hex(0xffffff),
        border: hex(0xe4e5ea),
        border_strong: hex(0xd2d4db),
        text: hex(0x1c1d21),
        text_dim: hex(0x5c606b),
        text_faint: hex(0x606572),
        accent: hex(0x4c58d2),
        accent_fg: hex(0xffffff),
        green: hex(0x0a7537),
        red: hex(0xc02a2a),
        yellow: hex(0x935700),
        blue: hex(0x1363c5),
        purple: hex(0x8045c7),
        orange: hex(0xb23f00),
        ansi: [
            hex(0x3a3d45), hex(0xd23c3c), hex(0x1f8a4c), hex(0xa66a00), hex(0x1f6fd1), hex(0x8a4fd1), hex(0x0f8a8a), hex(0x8f93a0),
            hex(0x5c606b), hex(0xe85a5a), hex(0x2ea862), hex(0xc58300), hex(0x3a86e8), hex(0xa26be6), hex(0x1ba6a6), hex(0x1c1d21),
        ],
    };

    pub const ALL: [Theme; 4] = [Theme::KILN_DARK, Theme::MIDNIGHT, Theme::EMBER, Theme::KILN_LIGHT];

    /// 기존 이름(다크 기본 테마).
    pub const DARK: Theme = Theme::KILN_DARK;

    pub fn current() -> Theme {
        Theme::ALL[CURRENT.load(Ordering::Relaxed).min(Theme::ALL.len() - 1)]
    }

    /// 이름으로 테마를 고른다. 모르는 이름이면 기본 테마.
    pub fn set_current(name: &str) {
        let i = Theme::ALL.iter().position(|t| t.name == name).unwrap_or(0);
        CURRENT.store(i, Ordering::Relaxed);
    }

    pub fn by_name(name: &str) -> Theme {
        Theme::ALL.iter().copied().find(|t| t.name == name).unwrap_or(Theme::KILN_DARK)
    }

    /// 강조색을 투명도와 함께.
    pub fn accent_soft(&self, alpha: u8) -> Color32 {
        Color32::from_rgba_unmultiplied(self.accent.r(), self.accent.g(), self.accent.b(), alpha)
    }

    pub fn shadow(&self) -> Shadow {
        Shadow { offset: [0, 8], blur: 28, spread: 0, color: Color32::from_black_alpha(if self.dark { 140 } else { 40 }) }
    }

    /// egui 전역 스타일에 테마와 타이포그래피를 적용한다.
    pub fn apply(&self, ctx: &egui::Context) {
        // Kiln's saved palette is an explicit choice. Do not let the OS switch
        // egui to an untouched style after startup or an appearance change.
        ctx.set_theme(if self.dark { egui::Theme::Dark } else { egui::Theme::Light });
        let mut v = if self.dark { egui::Visuals::dark() } else { egui::Visuals::light() };
        v.dark_mode = self.dark;
        v.panel_fill = self.bg_panel;
        v.window_fill = self.bg_elevated;
        v.window_stroke = Stroke::new(1.0, self.border_strong);
        v.window_corner_radius = CornerRadius::same(RADIUS_LG);
        v.window_shadow = self.shadow();
        v.popup_shadow = self.shadow();
        v.menu_corner_radius = CornerRadius::same(RADIUS);
        v.extreme_bg_color = self.bg_input;
        v.faint_bg_color = self.bg_elevated;
        v.code_bg_color = self.bg_elevated;
        v.selection.bg_fill = self.accent_soft(if self.dark { 90 } else { 60 });
        v.selection.stroke = Stroke::new(1.0, self.text);
        v.hyperlink_color = self.accent;
        v.text_cursor.stroke = Stroke::new(2.0, self.accent);
        v.override_text_color = Some(self.text);
        v.warn_fg_color = self.yellow;
        v.error_fg_color = self.red;
        v.striped = false;
        v.slider_trailing_fill = true;
        v.handle_shape = egui::style::HandleShape::Circle;
        let r = CornerRadius::same(RADIUS_SM + 1);
        let w = &mut v.widgets;
        w.noninteractive.bg_fill = self.bg_panel;
        w.noninteractive.weak_bg_fill = self.bg_panel;
        w.noninteractive.bg_stroke = Stroke::new(1.0, self.border);
        w.noninteractive.fg_stroke = Stroke::new(1.0, self.text);
        w.noninteractive.corner_radius = r;
        w.inactive.bg_fill = self.bg_hover;
        w.inactive.weak_bg_fill = self.bg_elevated;
        w.inactive.bg_stroke = Stroke::new(1.0, self.border_strong);
        w.inactive.fg_stroke = Stroke::new(1.0, self.text);
        w.inactive.corner_radius = r;
        w.inactive.expansion = 0.0;
        w.hovered.bg_fill = self.bg_selected;
        w.hovered.weak_bg_fill = self.bg_hover;
        w.hovered.bg_stroke = Stroke::new(1.0, self.border_strong);
        w.hovered.fg_stroke = Stroke::new(1.0, self.text);
        w.hovered.corner_radius = r;
        w.hovered.expansion = 0.0;
        w.active.bg_fill = self.bg_selected;
        w.active.weak_bg_fill = self.bg_selected;
        w.active.bg_stroke = Stroke::new(1.0, self.accent);
        // egui also uses active.text_color() for strong labels. Filled primary
        // buttons already select accent_fg in our explicit button primitive.
        w.active.fg_stroke = Stroke::new(1.0, self.text);
        w.active.corner_radius = r;
        w.active.expansion = 0.0;
        w.open.bg_fill = self.bg_selected;
        w.open.weak_bg_fill = self.bg_selected;
        w.open.bg_stroke = Stroke::new(1.0, self.border_strong);
        w.open.fg_stroke = Stroke::new(1.0, self.text);
        w.open.corner_radius = r;
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = Vec2::new(8.0, 6.0);
            s.spacing.button_padding = Vec2::new(10.0, 5.0);
            s.spacing.interact_size = Vec2::new(28.0, 26.0);
            s.spacing.menu_margin = Margin::same(6);
            s.spacing.window_margin = Margin::same(14);
            s.spacing.combo_height = 280.0;
            s.spacing.indent = 16.0;
            s.spacing.icon_width = 15.0;
            s.spacing.icon_spacing = 7.0;
            s.spacing.slider_rail_height = 4.0;
            s.spacing.scroll = egui::style::ScrollStyle::floating();
            s.spacing.scroll.bar_width = 8.0;
            s.spacing.scroll.floating_allocated_width = 0.0;
            s.interaction.selectable_labels = false;
            s.text_styles = [
                (TextStyle::Small, FontId::new(11.5, FontFamily::Proportional)),
                (TextStyle::Body, FontId::new(13.5, FontFamily::Proportional)),
                (TextStyle::Button, FontId::new(13.0, FontFamily::Proportional)),
                (TextStyle::Heading, FontId::new(18.0, FontFamily::Name(crate::fonts::SEMIBOLD.into()))),
                (TextStyle::Monospace, FontId::new(13.0, FontFamily::Monospace)),
            ]
            .into();
        });
    }
}

static CURRENT: AtomicUsize = AtomicUsize::new(0);

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn chosen_theme_survives_opposite_system_appearance_at_startup_and_later() {
        for chosen in Theme::ALL {
            for initial in [egui::Theme::Light, egui::Theme::Dark] {
                let ctx = egui::Context::default();
                let mut warmup = ctx.run_ui(egui::RawInput { system_theme: Some(initial), ..Default::default() }, |_| {});
                warmup.textures_delta.clear();
                chosen.apply(&ctx);
                for system in [initial, egui::Theme::Light, egui::Theme::Dark] {
                    let mut output = ctx.run_ui(egui::RawInput { system_theme: Some(system), ..Default::default() }, |ui| {
                        assert_eq!(ui.visuals().dark_mode, chosen.dark);
                        assert_eq!(ui.visuals().panel_fill, chosen.bg_panel);
                        assert_eq!(ui.visuals().widgets.inactive.bg_fill, chosen.bg_hover);
                        assert_eq!(ui.spacing().button_padding, Vec2::new(10.0, 5.0));
                        assert_eq!(ui.style().text_styles[&TextStyle::Button], FontId::new(13.0, FontFamily::Proportional));
                        let _ = ui.button("Appearance regression");
                    });
                    output.textures_delta.clear();
                }
            }
        }
    }
}

#[cfg(test)]
mod contrast_tests {
    use super::*;
    // WCAG 2.2 SC 1.4.3: normal-size text >= 4.5:1, without rounding.
    // https://www.w3.org/WAI/WCAG22/Understanding/contrast-minimum.html
    fn luminance(c:Color32)->f64 {
        let linear=|v:u8|{let x=f64::from(v)/255.0;if x<=0.04045{x/12.92}else{((x+0.055)/1.055).powf(2.4)}};
        0.2126*linear(c.r())+0.7152*linear(c.g())+0.0722*linear(c.b())
    }
    fn ratio(a:Color32,b:Color32)->f64 {
        let (a,b)=(luminance(a),luminance(b));(a.max(b)+0.05)/(a.min(b)+0.05)
    }
    #[test]
    fn strong_labels_and_pressed_widgets_remain_readable_in_every_theme() {
        let ctx=egui::Context::default();
        for t in Theme::ALL {
            t.apply(&ctx);
            let visuals=ctx.style_of(if t.dark {egui::Theme::Dark}else{egui::Theme::Light}).visuals.clone();
            for background in [t.bg,t.bg_panel,t.bg_elevated,visuals.widgets.active.bg_fill] {
                assert!(ratio(visuals.strong_text_color(),background)>=4.5,"{} strong text contrast",t.name);
            }
        }
    }
    #[test]
    fn normal_text_and_action_labels_meet_aa_on_all_shared_surfaces() {
        for t in Theme::ALL {
            for (role,fg) in [("text",t.text),("dim",t.text_dim),("faint",t.text_faint),("green",t.green),("red",t.red),("yellow",t.yellow),("blue",t.blue),("purple",t.purple),("orange",t.orange),("accent",t.accent)] {
                for bg in [t.bg,t.bg_panel,t.bg_elevated,t.bg_hover,t.bg_selected,t.bg_input] {
                    assert!(ratio(fg,bg)>=4.5,"{} {role}: {}",t.name,ratio(fg,bg));
                }
            }
            for (role,fg,bg) in [("primary",t.accent_fg,t.accent),("danger",if t.dark{t.accent_fg}else{Color32::WHITE},t.red)] {
                let hover_target=if role=="danger" && !t.dark {Color32::BLACK}else{Color32::WHITE};
                for bg in [bg,crate::widgets::lerp_color(bg,hover_target,0.08)] {
                    assert!(ratio(fg,bg)>=4.5,"{} {role}: {}",t.name,ratio(fg,bg));
                }
            }
        }
    }
}
