//! egui 单二进制主界面 —— 按 `design/ui-mockup.html` 视觉稿重绘。
//!
//! 0.2.0 起接入 `runner`：真正启动 / 停止 Python 生产引擎 `bridge_worker.py --live`，
//! 实时显示它的结构化日志；数据面板仍只读 `bridge.db`，两边互不写冲突。
//! 视觉实现约定：所有卡片 / 按钮 / 导航 / 图标 / 投屏外框都在 `widgets.rs` 手绘，
//! 颜色与尺寸只从 `Theme` 取，界面代码里不出现裸色值。

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod config;
mod datasource;
mod dbclean;
mod pages;
mod phone;
mod runner;
mod startup;
mod theme;
mod widgets;

use bridge_core::data::{self, FlowFilter, FlowRow, ReconcileRow, Snapshot};
use config::{ConfigDoc, RuleForm};
use eframe::egui::{self, Color32, RichText};
use std::time::{Duration, Instant};
use theme::Theme;
use widgets::{ButtonStyle, Icon};

const NAV: [&str; 5] = ["运行总览", "消息流水", "规则与会话", "漏转对账", "设置中心"];

/// 32×32 裸 RGBA，供 eframe 设置窗口/任务栏图标（exe 资源图标另由 build.rs 嵌入）。
const WINDOW_ICON: &[u8] = include_bytes!("../../../assets/app_32.rgba");

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RunState {
    Stopped,
    Starting,
    Running,
    Stopping,
    Checking,
    Failed,
}

impl RunState {
    fn busy(self) -> bool {
        matches!(self, Self::Starting | Self::Running | Self::Stopping | Self::Checking)
    }

    fn pill(self, t: &Theme) -> (Color32, Color32, &'static str) {
        match self {
            Self::Running => (t.success, t.success_soft, "转发运行中"),
            Self::Checking => (t.info, t.bg_subtle, "启动自检中"),
            Self::Stopping => (t.warning, t.warning_soft, "正在停止"),
            Self::Starting => (t.info, t.bg_subtle, "正在启动"),
            Self::Failed => (t.danger, t.danger_soft, "启动失败"),
            Self::Stopped => (t.text_faint, t.bg_subtle, "转发未启动"),
        }
    }
}

type LogLine = (Color32, String, String, String); // 颜色 / 级别 / 时间 / 文本

fn level_style(level: &str, t: &Theme) -> (Color32, String) {
    match level.to_ascii_uppercase().as_str() {
        "ERROR" | "CRITICAL" | "ERR" => (t.danger, "ERR".into()),
        "WARNING" | "WARN" => (t.warning, "WARN".into()),
        "DEBUG" => (t.text_faint, "DBG".into()),
        "OK" => (t.success, "OK".into()),
        _ => (t.info, "INFO".into()),
    }
}

/// 字段对 `pages.rs` 可见：四个管理页以 `impl BridgeApp` 的形式写在那个模块里。
struct BridgeApp {
    pub(crate) t: Theme,
    pub(crate) nav: usize,
    /// 本窗口事件 + worker 结构化日志（最新在末尾）。
    pub(crate) logs: Vec<LogLine>,
    /// 从 bridge.db 只读刷出来的数据快照。
    pub(crate) snapshot: Snapshot,
    pub(crate) last_refresh: Instant,
    pub(crate) refresh_every: Duration,
    /// Python 生产引擎定位结果。
    pub(crate) backend: Result<runner::Backend, String>,
    pub(crate) worker: Option<runner::Worker>,
    pub(crate) run_state: RunState,
    pub(crate) last_error: Option<String>,

    // ── 0.4.0 管理页状态 ────────────────────────────────────────────
    pub(crate) config: ConfigDoc,
    pub(crate) rule_form: RuleForm,
    pub(crate) flow_search: String,
    pub(crate) flow_filter: FlowFilter,
    pub(crate) flow_rows: Vec<FlowRow>,
    pub(crate) flow_selected: Option<usize>,
    pub(crate) flow_error: Option<String>,
    pub(crate) recon_rows: Vec<ReconcileRow>,
    pub(crate) recon_error: Option<String>,
    pub(crate) settings_text: String,
    pub(crate) settings_dirty: bool,
    pub(crate) keep_days: String,
    pub(crate) cleanup_plan: Option<dbclean::Plan>,
    pub(crate) cleanup_armed: bool,
    pub(crate) cleanup_result: Option<String>,

    // ── 0.5.0 手机画面（adb 抓帧） ──────────────────────────────────
    pub(crate) phone_feed: Option<phone::PhoneFeed>,
    phone_state: phone::DeviceState,
    phone_frame: Option<phone::PhoneFrame>,
    phone_tex: Option<egui::TextureHandle>,
    phone_last_frame: Option<Instant>,
}

impl BridgeApp {
    fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let t = Theme::light();
        theme::apply(&cc.egui_ctx, &t);
        theme::load_cjk_fonts(&cc.egui_ctx);

        let snapshot = datasource::load();
        let backend = runner::discover(snapshot.db_path.as_deref());
        let mut logs: Vec<LogLine> = Vec::new();

        match &snapshot.error {
            Some(e) => logs.push((t.warning, "WARN".into(), datasource::local_clock(), format!("数据源：{e}"))),
            None => logs.push((
                t.success,
                "OK".into(),
                datasource::local_clock(),
                format!(
                    "已只读接入 bridge.db · 累计 {} 条消息 / {} 次投递",
                    snapshot.kpi.total_messages,
                    snapshot.kpi.total_ok + snapshot.kpi.total_fail
                ),
            )),
        }
        match &backend {
            Ok(b) => {
                logs.push((
                    t.success,
                    "OK".into(),
                    datasource::local_clock(),
                    format!(
                        "{}已就绪：{}",
                        if b.bundled { "内置转发引擎（无需 Python 正式版）" } else { "转发引擎" },
                        b.root.display()
                    ),
                ));
                logs.push((
                    t.info,
                    "INFO".into(),
                    datasource::local_clock(),
                    "F5 开始转发 · F6 停止 · Ctrl+R 刷新数据".into(),
                ));
            }
            Err(e) => logs.push((t.danger, "ERR".into(), datasource::local_clock(), e.clone())),
        }

        let config = ConfigDoc::load();
        let rule_form = RuleForm::from_doc(&config);
        let settings_text = config.text.clone();
        let keep_days = config.get("storage", "keep_days").unwrap_or_default();
        // 手机画面：adb 在后台线程里抓帧，UI 只管收帧。找不到 adb 也要起
        // （线程自己会原地等待），这样插上手机后无需重启界面。
        let feed = phone::PhoneFeed::spawn();
        match &feed.adb_path {
            Some(p) => logs.push((
                t.info,
                "INFO".into(),
                datasource::local_clock(),
                format!("手机画面：adb 就绪（{p}），插上手机即可实时查看"),
            )),
            None => logs.push((
                t.warning,
                "WARN".into(),
                datasource::local_clock(),
                "手机画面：没找到 adb.exe（backend/tools/platform-tools），实时画面不可用".into(),
            )),
        }

        let mut app = Self {
            t,
            nav: 0,
            logs,
            snapshot,
            last_refresh: Instant::now(),
            refresh_every: Duration::from_secs(3),
            backend,
            worker: None,
            run_state: RunState::Stopped,
            last_error: None,
            config,
            rule_form,
            flow_search: String::new(),
            flow_filter: FlowFilter::All,
            flow_rows: Vec::new(),
            flow_selected: None,
            flow_error: None,
            recon_rows: Vec::new(),
            recon_error: None,
            settings_text,
            settings_dirty: false,
            keep_days: if keep_days.is_empty() { "90".to_owned() } else { keep_days },
            cleanup_plan: None,
            cleanup_armed: false,
            cleanup_result: None,
            phone_feed: None,
            phone_state: phone::DeviceState::Offline,
            phone_frame: None,
            phone_tex: None,
            phone_last_frame: None,
        };
        app.phone_feed = Some(feed);
        app.refresh_pages();
        app
    }

    pub(crate) fn push_log(&mut self, color: Color32, level: &str, text: impl Into<String>) {
        self.logs.push((color, level.to_owned(), datasource::local_clock(), text.into()));
        if self.logs.len() > 200 {
            self.logs.drain(..self.logs.len() - 160);
        }
    }

    pub(crate) fn refresh(&mut self, manual: bool) {
        self.snapshot = datasource::load();
        self.last_refresh = Instant::now();
        self.refresh_pages();
        if manual {
            self.push_log(self.t.success, "OK", "已刷新 bridge.db 数据（只读）");
        }
    }

    fn poll_worker(&mut self) {
        let events = self.worker.as_mut().map(|w| w.poll()).unwrap_or_default();
        for e in events {
            match e {
                runner::WorkerEvent::Log { time, level, text } => {
                    let (color, lvl) = level_style(&level, &self.t);
                    self.logs.push((color, lvl, time, text));
                }
                runner::WorkerEvent::Control { kind, detail } => match kind.as_str() {
                    "crash" => {
                        let brief: String = detail.lines().rev().take(4).collect::<Vec<_>>().join(" | ");
                        self.last_error = Some(format!("工作进程崩溃：{brief}"));
                        self.push_log(self.t.danger, "ERR", format!("工作进程崩溃：{brief}"));
                    }
                    "config-error" => {
                        self.push_log(self.t.warning, "WARN", format!("配置错误：{detail}"));
                    }
                    "exit" => self.push_log(self.t.info, "INFO", "工作进程返回"),
                    "start" => self.push_log(self.t.info, "INFO", "工作进程已拉起，正在初始化…"),
                    _ => {}
                },
                runner::WorkerEvent::Finished { code, graceful } => {
                    let was_check = self.worker.as_ref().map(|w| w.mode == runner::Mode::Check).unwrap_or(false);
                    self.worker = None;
                    if graceful || code == 0 {
                        self.run_state = RunState::Stopped;
                        self.push_log(
                            self.t.success,
                            "OK",
                            if was_check { "启动自检结束" } else { "转发服务已停止" },
                        );
                    } else {
                        self.run_state = RunState::Failed;
                        let msg = format!(
                            "工作进程异常退出（代码 {code}）。请查看下方日志与安装目录 startup.log。"
                        );
                        self.last_error = Some(msg.clone());
                        self.push_log(self.t.danger, "ERR", msg);
                    }
                }
            }
        }
        if self.logs.len() > 200 {
            self.logs.drain(..self.logs.len() - 160);
        }
    }

    pub(crate) fn start(&mut self, mode: runner::Mode) {
        if self.worker.is_some() {
            self.push_log(self.t.info, "INFO", "已有任务在运行，请先停止");
            return;
        }
        let backend = match &self.backend {
            Ok(b) => b.clone(),
            Err(e) => {
                self.run_state = RunState::Failed;
                self.last_error = Some(e.clone());
                self.push_log(self.t.danger, "ERR", e.clone());
                return;
            }
        };
        match runner::spawn(&backend, mode, self.snapshot.official_gui_open) {
            Ok(w) => {
                let pid = w.pid;
                self.worker = Some(w);
                self.run_state = if mode == runner::Mode::Live {
                    RunState::Running
                } else {
                    RunState::Checking
                };
                self.last_error = None;
                self.push_log(
                    self.t.success,
                    "OK",
                    format!(
                        "{}已启动（PID {pid}）· 引擎 {}",
                        if mode == runner::Mode::Live { "转发服务" } else { "启动自检" },
                        backend.source
                    ),
                );
                if mode == runner::Mode::Live {
                    self.push_log(
                        self.t.info,
                        "INFO",
                        format!("网关端口 {} · 识别到新消息会真实投递到微信", runner::GATEWAY_PORT),
                    );
                }
            }
            Err(e) => {
                self.run_state = RunState::Failed;
                self.last_error = Some(e.clone());
                for line in e.lines() {
                    self.push_log(self.t.danger, "ERR", line.to_owned());
                }
            }
        }
    }

    pub(crate) fn stop(&mut self) {
        if let Some(w) = self.worker.as_mut() {
            w.request_stop();
            self.run_state = RunState::Stopping;
            self.push_log(self.t.warning, "WARN", "已发出停止信号，等待工作进程收尾…");
        } else {
            self.run_state = RunState::Stopped;
            self.push_log(self.t.info, "INFO", "当前没有运行中的转发任务");
        }
    }
}

impl eframe::App for BridgeApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.poll_worker();

        let (f5, f6, ctrl_r) = ui.ctx().input(|i| {
            (
                i.key_pressed(egui::Key::F5),
                i.key_pressed(egui::Key::F6),
                i.modifiers.ctrl && i.key_pressed(egui::Key::R),
            )
        });
        if f5 {
            self.start(runner::Mode::Live);
        }
        if f6 {
            self.stop();
        }
        if ctrl_r || self.last_refresh.elapsed() >= self.refresh_every {
            self.refresh(ctrl_r);
        }
        ui.ctx().request_repaint_after(self.refresh_every);

        // 手机画面：排空抓帧通道，把最新一帧转成纹理（上一帧会被覆盖）。
        if let Some(feed) = &self.phone_feed {
            let mut state = self.phone_state.clone();
            let mut frame = self.phone_frame.take();
            phone::drain(&feed.rx, &mut state, &mut frame);
            self.phone_state = state;
            if let Some(f) = frame {
                let color = egui::ColorImage::from_rgba_unmultiplied(
                    [f.w as usize, f.h as usize],
                    &f.rgba,
                );
                match &self.phone_tex {
                    Some(tex) => tex.clone().set(color, egui::TextureOptions::LINEAR),
                    None => {
                        self.phone_tex = Some(ui.ctx().load_texture(
                            "phone_screen",
                            color,
                            egui::TextureOptions::LINEAR,
                        ));
                    }
                }
                self.phone_frame = Some(f);
                self.phone_last_frame = Some(Instant::now());
            }
        }

        // 快照成局部变量，避免各 Panel 闭包交叉借用 self。
        let t = self.t;
        let mut nav = self.nav;
        let logs = self.logs.clone();
        let snap = self.snapshot.clone();
        let backend = self.backend.clone();
        let run_state = self.run_state;
        let last_error = self.last_error.clone();
        let can_start = backend.is_ok() && !run_state.busy();
        let running = self.worker.is_some();
        let mut do_start = false;
        let mut do_stop = false;
        let mut do_check = false;
        let mut do_refresh = false;
        let mut do_open_settings = false;
        let mut do_phone_pause = false;
        let mut do_phone_kick = false;

        // 手机画面面板的快照（闭包里只用局部值）。
        let phone_state = self.phone_state.clone();
        let phone_tex = self.phone_tex.clone();
        let phone_size = self.phone_frame.as_ref().map(|f| (f.w as f32, f.h as f32));
        let phone_paused = self.phone_feed.as_ref().map(|f| f.is_paused()).unwrap_or(true);
        let phone_age = self
            .phone_last_frame
            .map(|i| i.elapsed().as_secs_f32());

        ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_app);

        // ── 自定义标题栏 ──────────────────────────────────────────────
        egui::Panel::top("titlebar").exact_size(t.titlebar_h).show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_surface);
            let drag = ui.interact(
                ui.max_rect(),
                egui::Id::new("titlebar_drag"),
                egui::Sense::drag(),
            );
            if drag.dragged() {
                ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
            }
            ui.horizontal(|ui| {
                ui.add_space(t.space_4);
                widgets::brand_mark(ui, &t);
                ui.add_space(t.space_2);
                ui.colored_label(
                    t.text_strong,
                    RichText::new("喵喵钉钉-微信消息转发桥").size(15.0).strong(),
                );
                widgets::pill(ui, &t, "独立版", t.primary, t.primary_soft, None);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(t.space_2);
                    if widgets::window_button(ui, &t, true).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    if widgets::window_button(ui, &t, false).clicked() {
                        ui.ctx().send_viewport_cmd(egui::ViewportCommand::Minimized(true));
                    }
                    ui.colored_label(
                        t.text_faint,
                        RichText::new("v0.5.0 · 独立版")
                            .size(11.0)
                            .family(egui::FontFamily::Monospace),
                    );
                });
            });
        });

        // ── 顶部状态条 ────────────────────────────────────────────────
        egui::Panel::top("topbar").exact_size(t.topbar_h).show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_topbar);
            ui.horizontal(|ui| {
                ui.add_space(t.space_4);
                let (fg, bg, txt) = run_state.pill(&t);
                widgets::pill(ui, &t, txt, fg, bg, Some(fg));
                ui.add_space(t.space_1);
                if datasource::has_bundled_backend() {
                    widgets::pill(ui, &t, "独立版 · 内置引擎", t.success, t.success_soft, Some(t.success));
                    ui.add_space(t.space_1);
                }
                if snap.error.is_some() {
                    widgets::pill(ui, &t, "数据未连接", t.warning, t.warning_soft, Some(t.warning));
                } else {
                    widgets::pill(ui, &t, "数据只读", t.info, t.bg_subtle, None);
                }
                ui.add_space(t.space_2);
                ui.colored_label(t.text_faint, "·");
                ui.colored_label(t.text_muted, RichText::new("当前会话").size(11.0));
                ui.colored_label(t.text, RichText::new(&snap.chat_name).size(13.0).strong());

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.add_space(t.space_4);
                    if widgets::action_button(
                        ui, &t, Icon::Bolt, "刷新数据", ButtonStyle::Quiet, true,
                    ).clicked() {
                        do_refresh = true;
                    }
                    ui.add_space(t.space_3);
                    ui.colored_label(
                        t.text_muted,
                        RichText::new(format!("今日转发 {}", snap.kpi.today_ok)).size(12.0),
                    );
                });
            });
        });

        // ── 左侧导航 ──────────────────────────────────────────────────
        egui::Panel::left("nav")
            .exact_size(t.sidebar_w)
            .resizable(false)
            .show(ui, |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_sidebar);
                ui.add_space(t.space_4);
                ui.colored_label(
                    t.text_faint,
                    RichText::new("工 作 台").size(11.0).family(egui::FontFamily::Monospace),
                );
                ui.add_space(t.space_2);

                let icons = [Icon::Grid, Icon::Chat, Icon::Sliders, Icon::CheckCircle, Icon::Gear];
                for (i, label) in NAV.iter().enumerate() {
                    if widgets::nav_item(ui, &t, icons[i], label, None, nav == i).clicked() {
                        nav = i;
                    }
                }

                ui.add_space(t.space_6);
                widgets::card(ui, &t, |ui| {
                    ui.horizontal(|ui| {
                        let (r, _) = ui.allocate_exact_size(egui::Vec2::new(18.0, 18.0), egui::Sense::hover());
                        widgets::icon(ui.painter(), r.center(), 16.0, t.info, Icon::Bolt);
                        ui.colored_label(t.text_strong, RichText::new("转发引擎").size(12.0).strong());
                    });
                    ui.add_space(t.space_1);
                    match &backend {
                        Ok(b) => ui.colored_label(t.text_muted, RichText::new(b.short()).size(11.0)),
                        Err(_) => ui.colored_label(t.danger, RichText::new("未找到后端").size(11.0)),
                    };
                    ui.colored_label(t.text_faint, RichText::new("F5 开始 · F6 停止").size(11.0));
                    ui.colored_label(t.text_faint, RichText::new("Ctrl+R 刷新数据").size(11.0));
                    ui.colored_label(t.text_faint, RichText::new("每 3 秒自动刷新").size(11.0));
                });
            });

        // ── 右侧手机画面 ──────────────────────────────────────────────
        // 宽度跟随窗口自适应（30%，300~560），不再固定 308：宽屏上把右侧
        // 空间用满，窄屏上也不挤中央区。画面等比撑满面板剩余高度。
        let phone_w = (ui.input(|i| i.viewport_rect().width()) * 0.30).clamp(300.0, 560.0);
        egui::Panel::right("phone")
            .exact_size(phone_w)
            .resizable(false)
            .show(ui, |ui| {
                ui.add_space(t.space_4);
                widgets::section_header(ui, &t, Icon::Phone, "手机画面", None);
                ui.add_space(t.space_2);

                // 状态行：设备状态 + 暂停/继续 + 手动刷新
                let (fg, bg, txt) = match &phone_state {
                    phone::DeviceState::Online => (t.success, t.success_soft, "设备在线"),
                    phone::DeviceState::Unauthorized => {
                        (t.danger, t.danger_soft, "USB 未授权")
                    }
                    phone::DeviceState::Offline => (t.warning, t.warning_soft, "未连接"),
                };
                ui.horizontal(|ui| {
                    widgets::pill(ui, &t, txt, fg, bg, Some(fg));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(t.space_1);
                        if widgets::action_button(
                            ui, &t, Icon::Bolt, "刷新", ButtonStyle::Quiet, true,
                        )
                        .clicked()
                        {
                            do_phone_kick = true;
                        }
                        ui.add_space(t.space_1);
                        let label = if phone_paused { "继续" } else { "暂停" };
                        if widgets::action_button(
                            ui, &t, Icon::Chart, label, ButtonStyle::Quiet, true,
                        )
                        .clicked()
                        {
                            do_phone_pause = true;
                        }
                    });
                });
                ui.add_space(t.space_2);

                // 画面区：等比撑满剩余高度（含 2 行页脚的预算）。
                let footer_h = 58.0;
                let avail = ui.available_height() - footer_h;
                let avail_w = ui.available_width() - t.space_2;
                let (rect, _) = ui.allocate_exact_size(
                    egui::Vec2::new(avail_w.max(1.0), avail.max(1.0)),
                    egui::Sense::hover(),
                );
                ui.painter()
                    .rect_filled(rect.expand(2.0), 8.0, t.bg_subtle);
                match (&phone_tex, phone_size) {
                    (Some(tex), Some((w, h))) if w > 0.0 && h > 0.0 => {
                        let scale =
                            (rect.width() / w).min(rect.height() / h);
                        let size = egui::Vec2::new(w * scale, h * scale);
                        let top_left =
                            rect.center() - size * 0.5;
                        egui::Image::new(tex)
                            .fit_to_exact_size(size)
                            .corner_radius(6.0)
                            .paint_at(ui, egui::Rect::from_min_size(top_left, size));
                        if let Some(age) = phone_age {
                            ui.painter().text(
                                rect.left_bottom() + egui::Vec2::new(8.0, -8.0),
                                egui::Align2::LEFT_BOTTOM,
                                format!("帧龄 {:.1}s", age),
                                egui::FontId::monospace(10.0),
                                t.text_faint,
                            );
                        }
                    }
                    _ => {
                        ui.painter().text(
                            rect.center(),
                            egui::Align2::CENTER_CENTER,
                            if phone_paused { "画面已暂停" } else { "等待手机画面…" },
                            egui::FontId::proportional(13.0),
                            t.text_faint,
                        );
                    }
                }
                ui.add_space(t.space_2);
                ui.horizontal(|ui| {
                    ui.colored_label(
                        t.text_faint,
                        RichText::new("面板走 adb，转发走 WiFi，互不影响").size(10.0),
                    );
                });
            });

        // ── 底部操作栏 ────────────────────────────────────────────────
        egui::Panel::bottom("bottombar")
            .exact_size(t.bottombar_h)
            .show(ui, |ui| {
                ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_surface);
                ui.horizontal(|ui| {
                    ui.add_space(t.space_4);
                    widgets::pill(ui, &t, "数据只读", t.info, t.bg_subtle, Some(t.info));
                    let src = snap.db_path.as_deref().map(datasource::display_path)
                        .unwrap_or_else(|| "未找到 bridge.db".to_owned());
                    ui.colored_label(t.text_faint, RichText::new(format!("数据源 {src}")).size(11.0));
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.add_space(t.space_3);
                        ui.colored_label(t.text_faint, RichText::new(format!("刷新于 {}", snap.refreshed_at)).size(11.0).family(egui::FontFamily::Monospace));
                        ui.add_space(t.space_3);
                        widgets::kbd(ui, &t, "Ctrl+R");
                        ui.colored_label(t.text_muted, RichText::new("刷新").size(11.0));
                        widgets::kbd(ui, &t, "F6");
                        ui.colored_label(t.text_muted, RichText::new("停止").size(11.0));
                        widgets::kbd(ui, &t, "F5");
                        ui.colored_label(t.text_muted, RichText::new("开始").size(11.0));
                    });
                });
            });

        // ── 中央主区 ──────────────────────────────────────────────────
        egui::CentralPanel::default().show(ui, |ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, t.bg_app);
            egui::ScrollArea::vertical().auto_shrink([false; 2]).show(ui, |ui| {
                ui.add_space(t.space_5);
                ui.horizontal(|ui| {
                    ui.colored_label(t.text_strong, RichText::new(NAV[nav]).size(22.0).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let (hint, color) = match &snap.error {
                            Some(e) => (format!("数据源异常 · {}", e.lines().next().unwrap_or("读取失败")), t.danger),
                            None => (format!("数据只读 · 每 3 秒刷新 · {}", snap.refreshed_at), t.text_faint),
                        };
                        ui.colored_label(color, RichText::new(hint).size(11.0));
                    });
                });
                ui.add_space(t.space_1);
                ui.colored_label(t.text_muted, RichText::new("钉钉 → 微信 消息转发桥 · 真实转发控制台").size(12.0));
                ui.add_space(t.space_3);

                // ── 运行控制条 ────────────────────────────────────────
                widgets::card(ui, &t, |ui| {
                    ui.horizontal(|ui| {
                        let (fg, _bg, label) = run_state.pill(&t);
                        ui.colored_label(fg, RichText::new(label).size(14.0).strong());
                        ui.add_space(t.space_2);
                        let desc = match &backend {
                            Ok(b) => format!("引擎 {} · 网关 {}", b.short(), runner::GATEWAY_PORT),
                            Err(e) => e.lines().next().unwrap_or("未找到引擎").to_owned(),
                        };
                        ui.colored_label(t.text_muted, RichText::new(desc).size(11.0));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let settings_ok = datasource::config_file().is_some()
                                || snap.official_exe.is_some();
                            if widgets::action_button(ui, &t, Icon::Gear, "打开配置", ButtonStyle::Quiet, settings_ok).clicked() {
                                do_open_settings = true;
                            }
                            ui.add_space(t.space_2);
                            if widgets::action_button(ui, &t, Icon::CheckCircle, "启动自检", ButtonStyle::Quiet, can_start).clicked() {
                                do_check = true;
                            }
                            ui.add_space(t.space_2);
                            if running {
                                if widgets::action_button(ui, &t, Icon::Stop, "停止转发", ButtonStyle::Danger, true).clicked() {
                                    do_stop = true;
                                }
                            } else if widgets::action_button(ui, &t, Icon::Play, "开始转发", ButtonStyle::Primary, can_start).clicked() {
                                do_start = true;
                            }
                        });
                    });
                });

                if let Some(e) = &last_error {
                    ui.add_space(t.space_2);
                    egui::Frame::default()
                        .fill(t.danger_soft)
                        .corner_radius(t.r_sm)
                        .inner_margin(t.space_3)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.colored_label(t.danger, RichText::new(e).size(12.0));
                        });
                }

                ui.add_space(t.space_4);

                if nav == 0 {
                // 4 张等宽 KPI 卡：口径与 Python 版 storage.stats() 对齐
                let k = &snap.kpi;
                let total_deliveries = k.total_ok + k.total_fail;
                let last_time = k.last_message_at.as_deref().map(data::hhmmss).unwrap_or_else(|| "—".to_owned());
                let last_date = k.last_message_at.as_deref().map(data::date_of).unwrap_or_else(|| "无记录".to_owned());
                ui.columns(4, |cols| {
                    let cards: [(Color32, &str, String, Icon, String); 4] = [
                        (t.primary, "今日转发", k.today_ok.to_string(), Icon::Chart, format!("累计入库 {} 条消息", k.total_messages)),
                        (t.success, "成功率", format!("{:.1}%", k.success_rate), Icon::CheckCircle, format!("累计 {} 成功 / {} 次投递", k.total_ok, total_deliveries)),
                        (t.warning, "投递失败", k.total_fail.to_string(), Icon::Bolt, "含重试记录，需人工确认".into()),
                        (t.info, "最后消息", last_time, Icon::Clock, last_date),
                    ];
                    for (i, (accent, label, value, ic, foot)) in cards.iter().enumerate() {
                        widgets::kpi_card(&mut cols[i], &t, label, value.as_str(), *accent, *ic, foot.as_str());
                    }
                });

                ui.add_space(t.space_4);

                // 实时日志：紧跟 KPI，最新在最上；固定高度 + 内部滚动，
                // 保证点「开始转发」后首屏就能看到引擎输出（不会被流水挤到窗口外）。
                widgets::card(ui, &t, |ui| {
                    widgets::section_header(ui, &t, Icon::Bolt, "实时日志",
                        Some("转发引擎输出 · 最新在最上"));
                    ui.add_space(t.space_2);
                    egui::Frame::default()
                        .fill(t.bg_subtle)
                        .corner_radius(t.r_sm)
                        .inner_margin(t.space_3)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            egui::ScrollArea::vertical()
                                .id_salt("live_log_scroll")
                                .max_height(140.0)
                                .auto_shrink([false; 2])
                                .show(ui, |ui| {
                                    ui.set_width(ui.available_width());
                                    if snap.entries.is_empty() && logs.is_empty() {
                                        ui.colored_label(t.text_faint, RichText::new("暂无日志，点「开始转发」后这里会实时输出").size(11.0));
                                    }
                                    for (color, level, time, line) in logs.iter().rev().take(60) {
                                        widgets::log_line(ui, &t, time, level, *color, line);
                                    }
                                    for e in snap.entries.iter().take(4) {
                                        let (color, level) = if e.ok { (t.success, "OK") } else { (t.warning, "WARN") };
                                        widgets::log_line(ui, &t, &e.time, level, color, &e.text);
                                    }
                                });
                        });
                });

                ui.add_space(t.space_2);

                // 消息流水：bridge.db 的真实记录
                widgets::card(ui, &t, |ui| {
                    let shown = snap.flow.len().min(6);
                    let hint = format!("最近 {shown} 条 · 累计 {} 条", k.total_messages);
                    widgets::section_header(ui, &t, Icon::Database, "消息流水", Some(&hint));
                    ui.add_space(t.space_2);
                    let w = ui.available_width();
                    let (hr, _) = ui.allocate_exact_size(egui::Vec2::new(w, 30.0), egui::Sense::hover());
                    ui.painter().rect_filled(hr, t.r_sm, t.bg_surface_2);
                    let col_x = [0.03, 0.16, 0.32, 0.84];
                    for (i, name) in ["时间", "发送者", "内容", "状态"].iter().enumerate() {
                        let x = hr.min.x + hr.width() * col_x[i];
                        ui.painter().text(egui::Pos2::new(x, hr.center().y), egui::Align2::LEFT_CENTER, *name,
                            egui::FontId::proportional(11.0), t.text_muted);
                    }
                    if snap.flow.is_empty() {
                        widgets::empty_state(ui, &t, "暂无消息流水", "开始转发后，识别到的消息会自动出现在这里");
                    } else {
                        for (i, row) in snap.flow.iter().take(6).enumerate() {
                            let (rect, _) = ui.allocate_exact_size(egui::Vec2::new(w, 28.0), egui::Sense::hover());
                            if i % 2 == 1 {
                                ui.painter().rect_filled(rect, 0.0, t.bg_subtle);
                            }
                            let p = ui.painter();
                            let x = col_x.map(|f| rect.min.x + rect.width() * f);
                            let cy = rect.center().y;
                            p.text(egui::Pos2::new(x[0], cy), egui::Align2::LEFT_CENTER, &row.time,
                                egui::FontId::monospace(11.0), t.text_muted);
                            p.text(egui::Pos2::new(x[1], cy), egui::Align2::LEFT_CENTER, &row.sender,
                                egui::FontId::proportional(11.0), t.text);
                            p.text(egui::Pos2::new(x[2], cy), egui::Align2::LEFT_CENTER, &row.summary,
                                egui::FontId::proportional(11.0), t.text_muted);
                            p.text(egui::Pos2::new(x[3], cy), egui::Align2::LEFT_CENTER, &row.status,
                                egui::FontId::proportional(11.0), if row.ok { t.success } else { t.warning });
                        }
                    }
                });

                ui.add_space(t.space_4);
                } else {
                    // 0.4.0：四个管理页接真实数据与配置，不再是空壳。
                    match nav {
                        1 => self.page_flow(ui),
                        2 => self.page_rules(ui),
                        3 => self.page_reconcile(ui),
                        _ => self.page_settings(ui),
                    }
                    ui.add_space(t.space_4);
                }
            });
        });

        // 写回交互产生的状态变更。
        if nav != self.nav {
            self.on_nav_change(nav);
        }
        self.nav = nav;
        if do_phone_pause || do_phone_kick {
            let mut paused_now = false;
            if let Some(feed) = &self.phone_feed {
                if do_phone_pause {
                    let new_state = !phone_paused;
                    feed.set_paused(new_state);
                    paused_now = true;
                }
                if do_phone_kick {
                    feed.kick();
                }
            }
            if paused_now {
                self.push_log(
                    self.t.info,
                    "INFO",
                    if phone_paused { "手机画面已恢复" } else { "手机画面已暂停（转发不受影响）" },
                );
            }
        }
        if do_refresh {
            self.refresh(true);
        }
        if do_start {
            self.start(runner::Mode::Live);
        }
        if do_check {
            self.start(runner::Mode::Check);
        }
        if do_stop {
            self.stop();
        }
        if do_open_settings {
            if let Some(cfg) = datasource::config_file() {
                if datasource::open_config() {
                    self.push_log(
                        self.t.success,
                        "OK",
                        format!("已打开配置文件 {cfg}（改完需 F6 停止、F5 重启引擎才生效）"),
                    );
                } else {
                    self.push_log(self.t.warning, "WARN", "打开配置文件失败，请到安装目录手动编辑 config.yaml");
                }
            } else if let Some(exe) = self.snapshot.official_exe.clone() {
                if datasource::open_official(&exe) {
                    self.push_log(self.t.success, "OK", "已打开正式版窗口（配置与高级设置在那里）");
                } else {
                    self.push_log(self.t.warning, "WARN", "打开正式版失败，请手动运行安装目录里的主程序");
                }
            }
        }
    }
}

/// 无人值守链路自检：`bridge-gui.exe --worker-smoke`。
///
/// 走与界面「开始转发」完全相同的 `runner::spawn()` 路径（同样的参数、环境变量、
/// CREATE_NO_WINDOW、管道读日志），确认网关端口真的起来、日志真的流回来，然后优雅停止。
/// GUI 子系统没有控制台，所有结论写进安装目录的 `startup.log`。
fn worker_smoke() -> i32 {
    let snap = datasource::load();
    let backend = match runner::discover(snap.db_path.as_deref()) {
        Ok(b) => b,
        Err(e) => {
            startup::log(&format!("SMOKE 结果 FAIL · 后端发现失败：{e}"));
            return 2;
        }
    };
    startup::log(&format!(
        "SMOKE 开始 · 引擎 {} · 解释器 {} · 配置 {}",
        backend.root.display(),
        backend.python.display(),
        backend.config.display()
    ));

    let mut worker = match runner::spawn(&backend, runner::Mode::Live, snap.official_gui_open) {
        Ok(w) => w,
        Err(e) => {
            startup::log(&format!("SMOKE 结果 FAIL · 启动失败：{e}"));
            return 3;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(25);
    let mut lines: usize = 0;
    let mut tail: Vec<String> = Vec::new();
    let mut listening = false;
    while Instant::now() < deadline {
        for e in worker.poll() {
            match e {
                runner::WorkerEvent::Log { level, text, .. } => {
                    lines += 1;
                    tail.push(format!("[{level}] {text}"));
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
                runner::WorkerEvent::Control { kind, detail } => {
                    tail.push(format!("<{kind}> {detail}"));
                    if tail.len() > 8 {
                        tail.remove(0);
                    }
                }
                runner::WorkerEvent::Finished { code, .. } => {
                    startup::log(&format!("SMOKE 结果 FAIL · 工作进程提前退出（代码 {code}）"));
                    for l in &tail {
                        startup::log(&format!("SMOKE 日志 {l}"));
                    }
                    return 4;
                }
            }
        }
        if runner::port_busy(runner::GATEWAY_PORT) {
            listening = true;
            if lines > 0 {
                break;
            }
        }
        std::thread::sleep(Duration::from_millis(250));
    }

    worker.request_stop();
    let stop_deadline = Instant::now() + Duration::from_secs(10);
    let mut stopped = false;
    while Instant::now() < stop_deadline {
        if worker.poll().iter().any(|e| matches!(e, runner::WorkerEvent::Finished { .. })) {
            stopped = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }

    for l in &tail {
        startup::log(&format!("SMOKE 日志 {l}"));
    }
    startup::log(&format!(
        "SMOKE 结果 {} · 网关 8765 监听={listening} · 收到日志 {lines} 行 · 优雅停止={stopped} · PID {}",
        if listening && lines > 0 { "PASS" } else { "FAIL" },
        worker.pid
    ));
    if listening && lines > 0 { 0 } else { 5 }
}

fn main() -> eframe::Result<()> {
    startup::install_panic_hook();
    let args: Vec<String> = std::env::args().skip(1).collect();
    startup::log(&format!(
        "启动 v{} | exe={:?} | args={:?}",
        env!("CARGO_PKG_VERSION"),
        std::env::current_exe().ok(),
        args
    ));

    if args.iter().any(|a| a == "--worker-smoke") {
        let code = worker_smoke();
        startup::log(&format!("SMOKE 退出码 {code}"));
        std::process::exit(code);
    }

    if startup::forward_to_existing() {
        return Ok(());
    }

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([1440.0, 900.0])
            .with_min_inner_size([1100.0, 720.0])
            .with_decorations(false)
            .with_icon(egui::IconData {
                rgba: WINDOW_ICON.to_vec(),
                width: 32,
                height: 32,
            })
            .with_title(startup::WINDOW_TITLE),
        ..Default::default()
    };
    startup::log("创建主窗口…");
    let result = eframe::run_native(
        startup::WINDOW_TITLE,
        options,
        Box::new(|cc| Ok(Box::new(BridgeApp::new(cc)))),
    );

    match &result {
        Ok(()) => startup::log("窗口正常关闭"),
        Err(e) => {
            let msg = format!("{e}");
            startup::log(&format!("窗口初始化失败：{msg}"));
            let detail = format!(
                "界面启动失败：\n\n{msg}\n\n日志：{}\n请把 startup.log 发给开发者。",
                startup::log_file("startup.log").display()
            );
            startup::message_box("启动失败", &detail, 0x10);
        }
    }
    result
}
