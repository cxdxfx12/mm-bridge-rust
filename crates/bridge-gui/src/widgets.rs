//! 视觉稿的自绘控件层。
//!
//! egui 原生控件只做骨架；卡片 / 按钮 / 导航 / 徽标 / 图标 / 手机外框都在这里手绘，
//! 颜色与尺寸一律取自 `Theme`（`design/ui-mockup.html` 的 `:root`），不写死色值。

use eframe::egui::{
    self, Align2, Color32, FontId, Pos2, Rect, RichText, Stroke, StrokeKind, Vec2,
};

use crate::theme::Theme;
use crate::PhoneState;

/// 线条图标（统一 1.6px 描边、圆头，和视觉稿的 .ic 规范一致）。
#[derive(Clone, Copy)]
pub enum Icon {
    Grid,
    Chat,
    Sliders,
    CheckCircle,
    Gear,
    Play,
    Stop,
    Phone,
    Clock,
    Chart,
    Info,
    Bolt,
    Wifi,
    Minus,
    Close,
    Database,
}

fn p2(x: f32, y: f32) -> Pos2 {
    Pos2::new(x, y)
}

/// 在给定中心画一个 size×size 的线条图标。
pub fn icon(p: &egui::Painter, c: Pos2, size: f32, color: Color32, kind: Icon) {
    let h = size / 2.0;
    let line = Stroke::new(1.6, color);
    let thin = Stroke::new(1.2, color);
    let seg = |a: Pos2, b: Pos2, s: Stroke| p.line_segment([a, b], s);
    let r = |x: f32, y: f32, w: f32, hh: f32, rad: f32, s: Stroke| {
        p.rect_stroke(Rect::from_min_size(p2(x, y), Vec2::new(w, hh)), rad, s, StrokeKind::Inside)
    };

    match kind {
        Icon::Grid => {
            for (dx, dy) in [(-h, -h), (h * 0.12, -h), (-h, h * 0.12), (h * 0.12, h * 0.12)] {
                r(c.x + dx, c.y + dy, size * 0.42, size * 0.42, 1.5, thin);
            }
        }
        Icon::Chat => {
            r(c.x - h, c.y - h * 0.8, size, size * 0.72, 2.5, line);
            seg(p2(c.x - h * 0.45, c.y + h * 0.05), p2(c.x + h * 0.45, c.y + h * 0.05), thin);
            seg(p2(c.x - h * 0.45, c.y + h * 0.38), p2(c.x + h * 0.12, c.y + h * 0.38), thin);
            seg(p2(c.x - h * 0.3, c.y + h * 0.72), p2(c.x - h * 0.55, c.y + h * 1.05), line);
        }
        Icon::Sliders => {
            for (i, dy) in [-h * 0.55, 0.0, h * 0.55].iter().enumerate() {
                let dy = *dy;
                seg(p2(c.x - h, c.y + dy), p2(c.x + h, c.y + dy), thin);
                let kx = c.x + [-h * 0.25, h * 0.35, -h * 0.05][i];
                p.rect_filled(
                    Rect::from_min_size(p2(kx - 2.5, c.y + dy - 3.5), Vec2::new(5.0, 7.0)),
                    1.5,
                    color,
                );
            }
        }
        Icon::CheckCircle => {
            r(c.x - h, c.y - h, size, size, 3.0, line);
            seg(p2(c.x - h * 0.45, c.y), p2(c.x - h * 0.05, c.y + h * 0.4), line);
            seg(p2(c.x - h * 0.05, c.y + h * 0.4), p2(c.x + h * 0.55, c.y - h * 0.45), line);
        }
        Icon::Gear => {
            r(c.x - h, c.y - h, size, size, 3.0, line);
            for dx in [-h * 0.4, 0.0, h * 0.4] {
                seg(p2(c.x + dx, c.y - h * 0.35), p2(c.x + dx, c.y + h * 0.35), thin);
            }
        }
        Icon::Play => {
            seg(p2(c.x - h * 0.4, c.y - h * 0.6), p2(c.x + h * 0.55, c.y), line);
            seg(p2(c.x + h * 0.55, c.y), p2(c.x - h * 0.4, c.y + h * 0.6), line);
            seg(p2(c.x - h * 0.4, c.y + h * 0.6), p2(c.x - h * 0.4, c.y - h * 0.6), line);
        }
        Icon::Stop => {
            p.rect_filled(
                Rect::from_min_size(p2(c.x - h * 0.5, c.y - h * 0.5), Vec2::splat(size * 0.5)),
                1.5,
                color,
            );
        }
        Icon::Phone => {
            r(c.x - h * 0.55, c.y - h, size * 0.55, size, 2.5, line);
            seg(p2(c.x - h * 0.15, c.y + h * 0.72), p2(c.x + h * 0.15, c.y + h * 0.72), thin);
        }
        Icon::Clock => {
            r(c.x - h, c.y - h, size, size, 3.0, line);
            seg(p2(c.x, c.y - h * 0.55), p2(c.x, c.y), line);
            seg(p2(c.x, c.y), p2(c.x + h * 0.45, c.y + h * 0.2), line);
        }
        Icon::Chart => {
            for dy in [-h * 0.6, 0.0, h * 0.6] {
                seg(p2(c.x - h, c.y + dy), p2(c.x + h, c.y + dy), thin);
            }
            seg(p2(c.x - h * 0.75, c.y + h * 0.45), p2(c.x - h * 0.15, c.y - h * 0.3), line);
            seg(p2(c.x - h * 0.15, c.y - h * 0.3), p2(c.x + h * 0.35, c.y + h * 0.1), line);
            seg(p2(c.x + h * 0.35, c.y + h * 0.1), p2(c.x + h * 0.8, c.y - h * 0.55), line);
        }
        Icon::Info => {
            r(c.x - h, c.y - h, size, size, 3.0, line);
            p.text(c, Align2::CENTER_CENTER, "i", FontId::proportional(size * 0.7), color);
        }
        Icon::Bolt => {
            seg(p2(c.x + h * 0.15, c.y - h), p2(c.x - h * 0.5, c.y + h * 0.1), line);
            seg(p2(c.x - h * 0.5, c.y + h * 0.1), p2(c.x + h * 0.05, c.y + h * 0.1), line);
            seg(p2(c.x + h * 0.05, c.y + h * 0.1), p2(c.x - h * 0.15, c.y + h), line);
        }
        Icon::Wifi => {
            for (i, w) in [0.4, 0.7, 1.0].iter().enumerate() {
                let dy = -h * 0.45 + i as f32 * h * 0.45;
                seg(p2(c.x - h * w, c.y + dy), p2(c.x + h * w, c.y + dy), thin);
            }
            p.circle_filled(p2(c.x, c.y + h * 0.6), 1.6, color);
        }
        Icon::Minus => {
            seg(p2(c.x - h * 0.5, c.y), p2(c.x + h * 0.5, c.y), line);
        }
        Icon::Close => {
            seg(p2(c.x - h * 0.45, c.y - h * 0.45), p2(c.x + h * 0.45, c.y + h * 0.45), line);
            seg(p2(c.x + h * 0.45, c.y - h * 0.45), p2(c.x - h * 0.45, c.y + h * 0.45), line);
        }
        Icon::Database => {
            for dy in [-h * 0.7, 0.0, h * 0.7] {
                r(c.x - h, c.y + dy - h * 0.3, size, h * 0.6, 2.0, thin);
            }
        }
    }
}

/// 柔和阴影卡片（视觉稿 `--shadow-sm` + `--r-md`）。
pub fn card<R>(
    ui: &mut egui::Ui,
    t: &Theme,
    content: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<R> {
    egui::Frame::default()
        .fill(t.bg_surface)
        .corner_radius(t.r_md)
        .stroke(Stroke::new(1.0, t.border))
        .inner_margin(t.space_4)
        .shadow(t.shadow_sm())
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            content(ui)
        })
}

/// 状态胶囊徽标。
pub fn pill(
    ui: &mut egui::Ui,
    _t: &Theme,
    text: &str,
    fg: Color32,
    bg: Color32,
    dot: Option<Color32>,
) {
    let extra = if dot.is_some() { 13.0 } else { 0.0 };
    let w = text.chars().count() as f32 * 7.0 + 22.0 + extra;
    let (rect, _) = ui.allocate_exact_size(Vec2::new(w, 24.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 999.0, bg);
    let mut x = rect.min.x + 11.0;
    if let Some(c) = dot {
        p.circle_filled(p2(x + 3.0, rect.center().y), 3.5, c);
        x += 13.0;
    }
    p.text(
        p2(x, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::proportional(11.0),
        fg,
    );
}

/// 快捷键键帽。
pub fn kbd(ui: &mut egui::Ui, t: &Theme, text: &str) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(24.0, 20.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, t.r_xs, t.bg_surface_2);
    p.rect_stroke(rect, t.r_xs, Stroke::new(1.0, t.border_strong), StrokeKind::Inside);
    p.text(
        rect.center(),
        Align2::CENTER_CENTER,
        text,
        FontId::monospace(10.0),
        t.text_muted,
    );
}

/// 区块标题：线性图标 + 标题 + 右侧说明。
pub fn section_header(ui: &mut egui::Ui, t: &Theme, kind: Icon, title: &str, hint: Option<&str>) {
    ui.horizontal(|ui| {
        let (r, _) = ui.allocate_exact_size(Vec2::new(20.0, 20.0), egui::Sense::hover());
        icon(ui.painter(), r.center(), 16.0, t.primary, kind);
        ui.colored_label(t.text_strong, RichText::new(title).size(14.0).strong());
        if let Some(h) = hint {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.colored_label(t.text_faint, RichText::new(h).size(11.0));
            });
        }
    });
}

/// KPI 卡片：顶部强调色条 + 右上线性图标 + 等宽大数字 + 脚注。
pub fn kpi_card(
    ui: &mut egui::Ui,
    t: &Theme,
    label: &str,
    value: &str,
    accent: Color32,
    kind: Icon,
    foot: &str,
) {
    card(ui, t, |ui| {
        // 色条画在卡片内容区顶部；画到 Frame 外框会落进 shadow 外边距里悬空。
        let r = ui.max_rect();
        ui.painter().rect_filled(
            Rect::from_min_size(r.min, Vec2::new(r.width(), 3.0)),
            2.0,
            accent,
        );
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.colored_label(t.text_muted, RichText::new(label).size(12.0));
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::new(20.0, 20.0), egui::Sense::hover());
                icon(ui.painter(), r.center(), 18.0, accent.gamma_multiply(0.75), kind);
            });
        });
        ui.add_space(2.0);
        ui.colored_label(
            accent,
            RichText::new(value)
                .size(28.0)
                .strong()
                .family(egui::FontFamily::Monospace),
        );
        ui.add_space(2.0);
        ui.colored_label(t.text_faint, RichText::new(foot).size(11.0));
    });
}

/// 左侧导航项：选中态淡蓝底 + 左侧主色竖条 + 图标 + 可选计数。
pub fn nav_item(
    ui: &mut egui::Ui,
    t: &Theme,
    kind: Icon,
    label: &str,
    badge: Option<&str>,
    selected: bool,
) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        Vec2::new(ui.available_width(), 38.0),
        egui::Sense::click(),
    );
    let p = ui.painter();
    let bg = if selected {
        t.primary_soft
    } else if resp.hovered() {
        t.bg_hover
    } else {
        Color32::TRANSPARENT
    };
    if bg != Color32::TRANSPARENT {
        p.rect_filled(rect, t.r_sm, bg);
    }
    if selected {
        p.rect_filled(
            Rect::from_min_size(rect.min + Vec2::new(0.0, 7.0), Vec2::new(3.0, rect.height() - 14.0)),
            2.0,
            t.primary,
        );
    }
    let fg = if selected { t.primary } else { t.text_muted };
    icon(p, p2(rect.min.x + 26.0, rect.center().y), 16.0, fg, kind);
    p.text(
        p2(rect.min.x + 46.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        if selected { t.primary } else { t.text },
    );
    if let Some(b) = badge {
        p.text(
            p2(rect.max.x - 20.0, rect.center().y),
            Align2::RIGHT_CENTER,
            b,
            FontId::monospace(11.0),
            t.text_faint,
        );
    }
    resp
}

#[derive(Clone, Copy)]
pub enum ButtonStyle {
    Primary,
    Quiet,
    Danger,
}

/// 主/次/危险按钮：圆角、描边、图标、hover/active 变色、禁用态。
pub fn action_button(
    ui: &mut egui::Ui,
    t: &Theme,
    kind: Icon,
    label: &str,
    style: ButtonStyle,
    enabled: bool,
) -> egui::Response {
    let (rect, resp) =
        ui.allocate_exact_size(Vec2::new(118.0, 36.0), egui::Sense::click());
    let p = ui.painter();
    let hover = resp.hovered() && enabled;
    let (fill, fg, border) = match (style, enabled) {
        (_, false) => (t.bg_subtle, t.text_faint, t.border),
        (ButtonStyle::Primary, true) => (
            if hover { t.primary_hover } else { t.primary },
            t.on_primary,
            t.primary,
        ),
        (ButtonStyle::Quiet, true) => (
            if hover { t.bg_hover } else { t.bg_surface },
            t.text,
            t.border_strong,
        ),
        (ButtonStyle::Danger, true) => (
            if hover { t.danger } else { t.danger_soft },
            if hover { t.on_primary } else { t.danger },
            t.danger,
        ),
    };
    p.rect_filled(rect, t.r_sm, fill);
    p.rect_stroke(rect, t.r_sm, Stroke::new(1.0, border), StrokeKind::Inside);
    icon(p, p2(rect.min.x + 21.0, rect.center().y), 14.0, fg, kind);
    p.text(
        p2(rect.min.x + 36.0, rect.center().y),
        Align2::LEFT_CENTER,
        label,
        FontId::proportional(13.0),
        fg,
    );
    resp
}

/// 日志行：等宽时间戳 + 彩色级别徽标 + 等宽文本。
pub fn log_line(ui: &mut egui::Ui, t: &Theme, time: &str, level: &str, color: Color32, text: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 24.0), egui::Sense::hover());
    let p = ui.painter();
    p.text(
        p2(rect.min.x + 2.0, rect.center().y),
        Align2::LEFT_CENTER,
        time,
        FontId::monospace(11.0),
        t.text_faint,
    );
    let badge = Rect::from_min_size(rect.min + Vec2::new(70.0, 4.0), Vec2::new(46.0, 16.0));
    p.rect_filled(badge, t.r_xs, color.gamma_multiply(0.18));
    p.text(
        badge.center(),
        Align2::CENTER_CENTER,
        level,
        FontId::proportional(10.0),
        color,
    );
    p.text(
        p2(rect.min.x + 126.0, rect.center().y),
        Align2::LEFT_CENTER,
        text,
        FontId::monospace(11.0),
        t.text,
    );
}

/// 空状态插画：避免骨架模式下中央区出现大片无意义留白。
pub fn empty_state(ui: &mut egui::Ui, t: &Theme, title: &str, hint: &str) {
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 132.0), egui::Sense::hover());
    let p = ui.painter();
    let c = rect.center() - Vec2::new(0.0, 18.0);
    p.circle_filled(c, 27.0, t.primary_soft);
    icon(p, c, 28.0, t.primary, Icon::Chat);
    p.text(
        rect.center() + Vec2::new(0.0, 22.0),
        Align2::CENTER_CENTER,
        title,
        FontId::proportional(13.0),
        t.text_muted,
    );
    p.text(
        rect.center() + Vec2::new(0.0, 44.0),
        Align2::CENTER_CENTER,
        hint,
        FontId::proportional(11.0),
        t.text_faint,
    );
}

/// 手机投屏面板：深色机身 + 圆角屏幕 + 状态栏 + 三态内容。
pub fn phone_mockup(ui: &mut egui::Ui, t: &Theme, state: PhoneState) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(232.0, 396.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 20.0, Color32::from_rgb(0x1C, 0x27, 0x40));
    let screen = rect.shrink(9.0);
    p.rect_filled(screen, 14.0, t.bg_surface);
    p.rect_filled(
        Rect::from_min_size(p2(rect.center().x - 18.0, rect.min.y + 4.0), Vec2::new(36.0, 3.0)),
        2.0,
        Color32::from_rgb(0x39, 0x44, 0x5E),
    );
    // 屏幕顶部状态栏
    p.text(
        p2(screen.min.x + 12.0, screen.min.y + 12.0),
        Align2::LEFT_CENTER,
        "9:41",
        FontId::monospace(9.0),
        t.text_muted,
    );
    icon(
        p,
        p2(screen.max.x - 18.0, screen.min.y + 12.0),
        10.0,
        t.text_muted,
        Icon::Wifi,
    );
    p.line_segment(
        [
            p2(screen.min.x, screen.min.y + 28.0),
            p2(screen.max.x, screen.min.y + 28.0),
        ],
        Stroke::new(1.0, t.border),
    );

    match state {
        PhoneState::Streaming => {
            // 模拟三条聊天气泡，证明「正常投屏」时画面长什么样。
            for (i, (w, left)) in [(140.0, true), (112.0, false), (128.0, true)]
                .iter()
                .enumerate()
            {
                let y = screen.min.y + 48.0 + i as f32 * 46.0;
                let x = if *left { screen.min.x + 14.0 } else { screen.max.x - w - 14.0 };
                let r = Rect::from_min_size(p2(x, y), Vec2::new(*w, 32.0));
                p.rect_filled(r, 8.0, if *left { t.bg_subtle } else { t.primary_soft });
                for k in 0..3 {
                    p.line_segment(
                        [
                            p2(r.min.x + 10.0, r.min.y + 10.0 + k as f32 * 7.0),
                            p2(r.max.x - 10.0, r.min.y + 10.0 + k as f32 * 7.0),
                        ],
                        Stroke::new(1.2, t.border_strong),
                    );
                }
            }
        }
        PhoneState::NoDevice | PhoneState::Unauthorized => {
            let c = screen.center();
            let col = state.color(t);
            p.circle_filled(c - Vec2::new(0.0, 30.0), 26.0, col.gamma_multiply(0.14));
            icon(p, c - Vec2::new(0.0, 30.0), 26.0, col, Icon::Phone);
            let (line1, line2) = if state == PhoneState::NoDevice {
                ("未检测到 adb 设备", "转发走 WiFi，不受影响")
            } else {
                ("USB 调试未授权", "请在手机上允许调试")
            };
            p.text(
                c + Vec2::new(0.0, 18.0),
                Align2::CENTER_CENTER,
                line1,
                FontId::proportional(12.0),
                col,
            );
            p.text(
                c + Vec2::new(0.0, 40.0),
                Align2::CENTER_CENTER,
                line2,
                FontId::proportional(10.0),
                t.text_faint,
            );
        }
    }
    // Home indicator
    p.rect_filled(
        Rect::from_min_size(
            p2(rect.center().x - 28.0, rect.max.y - 8.0),
            Vec2::new(56.0, 3.0),
        ),
        2.0,
        Color32::from_rgb(0x5C, 0x68, 0x84),
    );
}

/// 标题栏窗口按钮（最小化 / 关闭），hover 时高亮，关闭为危险色。
pub fn window_button(ui: &mut egui::Ui, t: &Theme, close: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(38.0, 30.0), egui::Sense::click());
    let p = ui.painter();
    let hover = resp.hovered();
    if hover {
        p.rect_filled(rect, t.r_xs, if close { t.danger } else { t.bg_hover });
    }
    let fg = if hover && close { t.on_primary } else { t.text_muted };
    icon(
        p,
        rect.center(),
        12.0,
        fg,
        if close { Icon::Close } else { Icon::Minus },
    );
    resp
}

/// 品牌方块：主色圆角底 + 白色信封线条。
pub fn brand_mark(ui: &mut egui::Ui, t: &Theme) {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(28.0, 28.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, t.r_sm, t.primary);
    icon(p, rect.center(), 16.0, t.on_primary, Icon::Bolt);
}
