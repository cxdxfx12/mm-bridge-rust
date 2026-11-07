//! 设计 token：把 `design/ui-mockup.html` 的 `:root` 变量翻译成 Rust。
//!
//! 约定：界面代码只引用这里的语义字段，禁止写死色值 / 尺寸。
//! 换深浅色只需替换一组变量；后续接系统主题时在这里做桥接。

use eframe::egui::{self, Color32, Stroke, Vec2};

fn rgb(hex: u32) -> Color32 {
    Color32::from_rgb(
        ((hex >> 16) & 0xff) as u8,
        ((hex >> 8) & 0xff) as u8,
        (hex & 0xff) as u8,
    )
}

/// 与视觉稿 `:root` 一一对应的浅色主题 token 集。
#[derive(Clone, Copy)]
pub struct Theme {
    pub bg_app: Color32,
    pub bg_surface: Color32,
    pub bg_surface_2: Color32,
    pub bg_subtle: Color32,
    pub bg_hover: Color32,
    pub bg_sidebar: Color32,
    pub bg_topbar: Color32,

    pub text_strong: Color32,
    pub text: Color32,
    pub text_muted: Color32,
    pub text_faint: Color32,
    pub border: Color32,
    pub border_strong: Color32,

    pub primary: Color32,
    pub primary_hover: Color32,
    pub primary_active: Color32,
    pub primary_soft: Color32,
    pub on_primary: Color32,
    pub success: Color32,
    pub success_soft: Color32,
    pub warning: Color32,
    pub warning_soft: Color32,
    pub danger: Color32,
    pub danger_soft: Color32,
    pub info: Color32,

    // 间距梯度（4px 基准）
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub space_5: f32,
    pub space_6: f32,
    pub space_8: f32,
    // 圆角
    pub r_xs: f32,
    pub r_sm: f32,
    pub r_md: f32,
    pub r_lg: f32,
    // 固定栅格
    pub control_h: f32,
    pub titlebar_h: f32,
    pub topbar_h: f32,
    pub bottombar_h: f32,
    pub sidebar_w: f32,
    pub phone_w: f32,
}

impl Theme {
    pub fn light() -> Self {
        Self {
            bg_app: rgb(0xEEF1F7),
            bg_surface: rgb(0xFFFFFF),
            bg_surface_2: rgb(0xF8FAFD),
            bg_subtle: rgb(0xF1F4F9),
            bg_hover: rgb(0xF1F5FC),
            bg_sidebar: rgb(0xF7F9FC),
            bg_topbar: rgb(0xFBFCFE),

            text_strong: rgb(0x16203A),
            text: rgb(0x1C2740),
            text_muted: rgb(0x5C6884),
            text_faint: rgb(0x8A96AE),
            border: rgb(0xE2E7F0),
            border_strong: rgb(0xCBD5E6),

            primary: rgb(0x2F6BED),
            primary_hover: rgb(0x2456C8),
            primary_active: rgb(0x1B419B),
            primary_soft: rgb(0xEEF3FE),
            on_primary: rgb(0xFFFFFF),
            success: rgb(0x0F9D58),
            success_soft: rgb(0xE9F7EF),
            warning: rgb(0xD99000),
            warning_soft: rgb(0xFFF5DA),
            danger: rgb(0xE5484D),
            danger_soft: rgb(0xFDECEC),
            info: rgb(0x5C6884),

            space_1: 4.0,
            space_2: 8.0,
            space_3: 12.0,
            space_4: 16.0,
            space_5: 20.0,
            space_6: 24.0,
            space_8: 32.0,
            r_xs: 4.0,
            r_sm: 6.0,
            r_md: 10.0,
            r_lg: 14.0,
            control_h: 34.0,
            titlebar_h: 46.0,
            topbar_h: 68.0,
            bottombar_h: 58.0,
            sidebar_w: 216.0,
            phone_w: 308.0,
        }
    }

    /// 深色变体：阴影在深色下失效，改用提亮描边（视觉稿规范）。
    #[allow(dead_code)]
    pub fn dark() -> Self {
        let mut t = Self::light();
        t.bg_app = rgb(0x111827);
        t.bg_surface = rgb(0x1C2740);
        t.bg_surface_2 = rgb(0x243350);
        t.bg_subtle = rgb(0x2C3B58);
        t.bg_hover = rgb(0x334563);
        t.bg_sidebar = rgb(0x18233A);
        t.bg_topbar = rgb(0x1C2740);
        t.text_strong = rgb(0xFFFFFF);
        t.text = rgb(0xE6ECF7);
        t.text_muted = rgb(0xA7B2C6);
        t.text_faint = rgb(0x7A869E);
        t.border = rgb(0x39445E);
        t.border_strong = rgb(0x5C6884);
        t.primary = rgb(0x6E9BFF);
        t.primary_soft = rgb(0x243B68);
        t
    }

    /// 视觉稿 `--shadow-sm`：柔和、Y 向偏移、不用重黑。
    pub fn shadow_sm(&self) -> egui::Shadow {
        egui::Shadow {
            offset: [0, 2],
            blur: 12,
            spread: 0,
            color: Color32::from_black_alpha(22),
        }
    }
}

/// 把 token 灌进 egui 的 Visuals，让下拉框 / 输入框等原生控件也贴合设计稿。
pub fn apply(ctx: &egui::Context, t: &Theme) {
    let mut style = egui::Style::default();
    style.visuals = egui::Visuals::light();
    style.visuals.window_fill = t.bg_app;
    style.visuals.panel_fill = t.bg_surface;
    style.visuals.extreme_bg_color = t.bg_surface_2;
    style.visuals.window_shadow = egui::Shadow {
        offset: [0, 8],
        blur: 32,
        spread: 0,
        color: Color32::from_black_alpha(20),
    };
    style.visuals.popup_shadow = t.shadow_sm();
    style.visuals.window_corner_radius = egui::CornerRadius::from(t.r_lg as u8);

    // 原生控件统一圆角（egui 0.37 用 CornerRadius，不再是 Rounding）。
    let cr = egui::CornerRadius::from(t.r_sm as u8);
    style.visuals.widgets.noninteractive.corner_radius = cr;
    style.visuals.widgets.inactive.corner_radius = cr;
    style.visuals.widgets.hovered.corner_radius = cr;
    style.visuals.widgets.active.corner_radius = cr;

    style.visuals.widgets.noninteractive.bg_fill = t.bg_surface_2;
    style.visuals.widgets.inactive.bg_fill = t.bg_surface_2;
    style.visuals.widgets.hovered.bg_fill = t.bg_hover;
    style.visuals.widgets.active.bg_fill = t.primary_soft;
    style.visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, t.border);
    style.visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, t.border_strong);
    style.visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, t.primary);
    style.visuals.selection.bg_fill = t.primary_soft;
    style.visuals.selection.stroke = Stroke::new(1.0, t.primary);
    style.visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, t.text);
    style.visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, t.text);

    style.spacing.item_spacing = Vec2::new(t.space_3, t.space_3);
    style.spacing.button_padding = Vec2::new(t.space_4, t.space_2);
    style.spacing.window_margin = egui::Margin::from(t.space_4);
    ctx.set_global_style(style);
}

/// egui 内置字体不含中文字形，必须运行时挂载系统字体，否则全是豆腐块。
/// 等宽族优先 Consolas（数字/时间戳对齐），中文用微软雅黑兜底。
pub fn load_cjk_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut loaded: Vec<&str> = Vec::new();

    for (name, path) in [
        ("mono", "C:/Windows/Fonts/consola.ttf"),
        ("cjk", "C:/Windows/Fonts/msyh.ttc"),
    ] {
        if let Ok(bytes) = std::fs::read(path) {
            fonts
                .font_data
                .insert(name.to_owned(), egui::FontData::from_owned(bytes).into());
            loaded.push(name);
        }
    }
    if loaded.contains(&"cjk") {
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, "cjk".to_owned());
    }
    if loaded.contains(&"mono") {
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, "mono".to_owned());
        if loaded.contains(&"cjk") {
            fonts
                .families
                .entry(egui::FontFamily::Monospace)
                .or_default()
                .push("cjk".to_owned());
        }
    } else if loaded.contains(&"cjk") {
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .push("cjk".to_owned());
    }
    ctx.set_fonts(fonts);
}
