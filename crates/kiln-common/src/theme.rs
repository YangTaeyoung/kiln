use egui::Color32;

/// 앱 전체 색상 팔레트 (다크).
#[derive(Clone, Copy, Debug)]
pub struct Theme {
    pub bg: Color32,
    pub bg_panel: Color32,
    pub bg_elevated: Color32,
    pub bg_hover: Color32,
    pub bg_selected: Color32,
    pub border: Color32,
    pub text: Color32,
    pub text_dim: Color32,
    pub text_faint: Color32,
    pub accent: Color32,
    pub green: Color32,
    pub red: Color32,
    pub yellow: Color32,
    pub blue: Color32,
    pub purple: Color32,
    pub orange: Color32,
}

impl Theme {
    pub const DARK: Theme = Theme {
        bg: Color32::from_rgb(0x16, 0x17, 0x1c),
        bg_panel: Color32::from_rgb(0x1c, 0x1d, 0x23),
        bg_elevated: Color32::from_rgb(0x24, 0x26, 0x2e),
        bg_hover: Color32::from_rgb(0x2b, 0x2d, 0x37),
        bg_selected: Color32::from_rgb(0x2f, 0x3a, 0x55),
        border: Color32::from_rgb(0x2e, 0x30, 0x3a),
        text: Color32::from_rgb(0xdc, 0xde, 0xe6),
        text_dim: Color32::from_rgb(0x9a, 0x9e, 0xae),
        text_faint: Color32::from_rgb(0x66, 0x6a, 0x7a),
        accent: Color32::from_rgb(0x6c, 0x9e, 0xff),
        green: Color32::from_rgb(0x7e, 0xc6, 0x7a),
        red: Color32::from_rgb(0xf0, 0x6c, 0x75),
        yellow: Color32::from_rgb(0xe5, 0xc0, 0x7b),
        blue: Color32::from_rgb(0x61, 0xaf, 0xef),
        purple: Color32::from_rgb(0xc6, 0x78, 0xdd),
        orange: Color32::from_rgb(0xe0, 0x96, 0x5c),
    };

    pub fn current() -> Theme {
        Theme::DARK
    }

    /// egui 전역 스타일에 팔레트를 적용한다.
    pub fn apply(&self, ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.panel_fill = self.bg_panel;
        v.window_fill = self.bg_elevated;
        v.extreme_bg_color = self.bg;
        v.faint_bg_color = self.bg_elevated;
        v.code_bg_color = self.bg_elevated;
        v.selection.bg_fill = self.bg_selected;
        v.selection.stroke.color = self.accent;
        v.hyperlink_color = self.accent;
        v.window_stroke.color = self.border;
        v.widgets.noninteractive.bg_stroke.color = self.border;
        v.widgets.noninteractive.fg_stroke.color = self.text;
        v.widgets.inactive.fg_stroke.color = self.text;
        v.widgets.inactive.bg_fill = self.bg_elevated;
        v.widgets.inactive.weak_bg_fill = self.bg_elevated;
        v.widgets.hovered.bg_fill = self.bg_hover;
        v.widgets.hovered.weak_bg_fill = self.bg_hover;
        v.widgets.active.bg_fill = self.bg_selected;
        v.widgets.active.weak_bg_fill = self.bg_selected;
        v.override_text_color = Some(self.text);
        ctx.set_visuals(v);
        ctx.global_style_mut(|s| {
            s.spacing.item_spacing = egui::vec2(6.0, 4.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
        });
    }
}
