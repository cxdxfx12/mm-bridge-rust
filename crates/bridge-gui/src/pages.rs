//! 四个管理页：消息流水 / 规则与会话 / 漏转对账 / 设置中心。
//!
//! 0.3.0 之前这四项只是换标题的空壳；本模块把它们接到真实的只读查询、
//! `config.yaml` 读写与数据库瘦身上。视觉元素一律复用 `widgets.rs`，
//! 数据写入只在「独立版自带库 + 引擎已停止」时放开。

use std::fs;

use bridge_core::data::{self, FlowFilter, FlowRow, ReconcileRow};
use eframe::egui::{self, Color32, RichText};

use crate::config::{ConfigSummary, RouteView, RuleForm};
use crate::{datasource, dbclean, runner, startup, widgets};
use widgets::{ButtonStyle, Icon, NoticeKind};

use super::BridgeApp;

const FLOW_LIMIT: i64 = 200;
const RECON_LIMIT: i64 = 200;

impl BridgeApp {
    // ────────────────────────── 数据刷新 ──────────────────────────

    pub(crate) fn reload_flow(&mut self) {
        match datasource::load_flow(FLOW_LIMIT, &self.flow_search, self.flow_filter) {
            Ok(rows) => {
                self.flow_rows = rows;
                self.flow_error = None;
            }
            Err(e) => {
                self.flow_rows.clear();
                self.flow_error = Some(e);
            }
        }
    }

    pub(crate) fn reload_reconcile(&mut self) {
        match datasource::load_reconcile(RECON_LIMIT) {
            Ok(rows) => {
                self.recon_rows = rows;
                self.recon_error = None;
            }
            Err(e) => {
                self.recon_rows.clear();
                self.recon_error = Some(e);
            }
        }
    }

    pub(crate) fn refresh_pages(&mut self) {
        self.reload_flow();
        self.reload_reconcile();
    }

    /// 切换页签时按需刷新；未保存的原文编辑不会被覆盖。
    pub(crate) fn on_nav_change(&mut self, next: usize) {
        match next {
            1 | 3 => self.refresh_pages(),
            2 | 4 => {
                if !self.settings_dirty {
                    self.config.reload();
                    self.settings_text = self.config.text.clone();
                    self.rule_form = RuleForm::from_doc(&self.config);
                }
            }
            _ => {}
        }
    }

    fn export(&mut self, name: &str, body: &str) {
        let path = datasource::exports_dir().join(name);
        let mut bytes = vec![0xEF, 0xBB, 0xBF]; // Excel 中文不乱码
        bytes.extend_from_slice(body.as_bytes());
        match fs::write(&path, bytes) {
            Ok(()) => {
                let shown = path.display().to_string();
                if datasource::open_path(&path) {
                    self.push_log(self.t.success, "OK", format!("已导出并打开 {shown}"));
                } else {
                    self.push_log(self.t.success, "OK", format!("已导出 {shown}"));
                }
            }
            Err(e) => self.push_log(self.t.danger, "ERR", format!("导出失败：{e}")),
        }
    }

    fn keep_value(&self) -> Result<i64, String> {
        let v = self.keep_days.trim().parse::<i64>().unwrap_or(dbclean::DEFAULT_KEEP_DAYS);
        if v < 0 || (v > 0 && v < dbclean::MIN_KEEP_DAYS) {
            Err(format!(
                "保留天数不能是负数，也不能少于 {} 天",
                dbclean::MIN_KEEP_DAYS
            ))
        } else {
            Ok(v)
        }
    }

    // ────────────────────────── 1. 消息流水 ──────────────────────────

    pub(crate) fn page_flow(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let mut search = self.flow_search.clone();
        let mut filter = self.flow_filter;
        let mut selected = self.flow_selected;
        let rows: Vec<FlowRow> = self.flow_rows.clone();
        let err = self.flow_error.clone();
        let old_search = search.clone();
        let old_filter = filter;
        let mut do_refresh = false;
        let mut do_export = false;

        widgets::card(ui, &t, |ui| {
            widgets::section_header(
                ui,
                &t,
                Icon::Chat,
                "消息流水",
                Some(&format!("最多显示 {FLOW_LIMIT} 条 · 来源 bridge.db（只读）")),
            );
            ui.add_space(t.space_2);
            ui.horizontal(|ui| {
                ui.add_sized(
                    egui::Vec2::new(260.0, 28.0),
                    egui::TextEdit::singleline(&mut search)
                        .hint_text("搜索发送者 / 正文 / 目标群")
                        .font(egui::FontId::proportional(12.0)),
                );
                ui.add_space(t.space_2);
                egui::ComboBox::from_id_salt("flow_status")
                    .selected_text(filter.label())
                    .width(120.0)
                    .show_ui(ui, |ui| {
                        for f in [
                            FlowFilter::All,
                            FlowFilter::Ok,
                            FlowFilter::Fail,
                            FlowFilter::Pending,
                        ] {
                            ui.selectable_value(&mut filter, f, f.label());
                        }
                    });
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Bolt, "刷新", ButtonStyle::Quiet, true).clicked() {
                    do_refresh = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Database, "导出 CSV", ButtonStyle::Quiet, !rows.is_empty()).clicked() {
                    do_export = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(t.text_faint, RichText::new(format!("命中 {} 条", rows.len())).size(11.0));
                });
            });
            ui.add_space(t.space_3);

            if let Some(e) = &err {
                widgets::notice(ui, &t, NoticeKind::Danger, "查询失败", e);
                ui.add_space(t.space_2);
            }

            widgets::table_header(
                ui,
                &t,
                &["时间", "发送者", "目标", "内容", "状态"],
                &[0.02, 0.20, 0.34, 0.50, 0.90],
            );
            egui::ScrollArea::vertical()
                .id_salt("flow_table_scroll")
                .max_height(330.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        widgets::empty_state(
                            ui,
                            &t,
                            "没有符合条件的消息",
                            "开始转发后这里会列出真实入库记录，可改关键词或筛选条件",
                        );
                    }
                    for (i, row) in rows.iter().enumerate() {
                        let (rect, resp) = widgets::table_row(ui, &t, i, 28.0, selected == Some(i));
                        if resp.clicked() {
                            selected = if selected == Some(i) { None } else { Some(i) };
                        }
                        let status_color = if row.ok {
                            t.success
                        } else if row.status == "投递失败" {
                            t.danger
                        } else {
                            t.warning
                        };
                        widgets::cell(ui, rect, 0.02, &row.when, t.text_muted, true);
                        widgets::cell(ui, rect, 0.20, &row.sender, t.text, false);
                        widgets::cell(ui, rect, 0.34, if row.target.is_empty() { "—" } else { &row.target }, t.text_muted, false);
                        widgets::cell(ui, rect, 0.50, &row.summary, t.text_muted, false);
                        widgets::cell(ui, rect, 0.90, &row.status, status_color, false);
                    }
                });
        });

        if let Some(i) = selected {
            if let Some(row) = rows.get(i) {
                ui.add_space(t.space_3);
                widgets::card(ui, &t, |ui| {
                    widgets::section_header(ui, &t, Icon::Info, "消息详情", Some("点击行可切换 / 再点一次收起"));
                    ui.add_space(t.space_2);
                    widgets::kv(ui, &t, "时间", &row.when, t.text);
                    widgets::kv(ui, &t, "发送者", &row.sender, t.text);
                    widgets::kv(ui, &t, "目标会话", if row.target.is_empty() { "—" } else { &row.target }, t.text);
                    widgets::kv(ui, &t, "消息类型", if row.msg_type.is_empty() { "—" } else { &row.msg_type }, t.text);
                    ui.add_space(t.space_2);
                    egui::Frame::default()
                        .fill(t.bg_subtle)
                        .corner_radius(t.r_sm)
                        .inner_margin(t.space_3)
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.colored_label(
                                t.text,
                                RichText::new(if row.full.is_empty() { "(无文本内容)" } else { &row.full }).size(12.0),
                            );
                        });
                });
            }
        }

        let filters_changed = search != old_search || filter != old_filter;
        self.flow_search = search;
        self.flow_filter = filter;
        self.flow_selected = selected;
        if do_export {
            self.export("消息流水.csv", &data::flow_csv(&rows));
        }
        if do_refresh || filters_changed {
            self.reload_flow();
            if do_refresh {
                self.push_log(self.t.success, "OK", "消息流水已重新查询（只读）");
            }
        }
    }

    // ────────────────────────── 2. 规则与会话 ──────────────────────────

    pub(crate) fn page_rules(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let summary: ConfigSummary = self.config.summary();
        let routes: Vec<RouteView> = self.config.routes();
        let mut form = self.rule_form.clone();
        let cfg_exists = self.config.exists();
        let cfg_err = self.config.error.clone();
        let mut do_save = false;
        let mut do_reload = false;
        let mut do_open = false;

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Info, "当前生效配置", Some("读取安装目录 config.yaml"));
            ui.add_space(t.space_2);
            ui.columns(2, |cols| {
                let left: Vec<(&str, String)> = vec![
                    ("采集模式", dash(&summary.mode)),
                    ("时间过滤", dash(&summary.time_filter)),
                    ("会话白名单", dash(&summary.chat_names)),
                    ("公司过滤", dash(&summary.companies)),
                    ("网关地址", format!("{}:{}", dash(&summary.gateway_host), dash(&summary.gateway_port))),
                    ("网关 token", if summary.gateway_token.is_empty() { "未配置".to_owned() } else { "已配置".to_owned() }),
                ];
                let right: Vec<(&str, String)> = vec![
                    ("微信驱动", dash(&summary.driver)),
                    ("演练模式", dash(&summary.dry_run)),
                    ("发送间隔", format!("{} 秒", dash(&summary.interval))),
                    ("积压上限", format!("{} 分钟", dash(&summary.max_backlog_min))),
                    ("数据保留", if summary.keep_days.is_empty() { "未设置".to_owned() } else { format!("{} 天", summary.keep_days) }),
                    ("通知 / 对账", format!("{} / {}", dash(&summary.notify_enabled), dash(&summary.reconcile_enabled))),
                ];
                for (label, value) in &left {
                    widgets::kv(&mut cols[0], &t, label, value, t.text);
                }
                for (label, value) in &right {
                    widgets::kv(&mut cols[1], &t, label, value, t.text);
                }
            });
        });

        ui.add_space(t.space_3);

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Sliders, "采集与投递规则", Some("改完点保存，重启引擎后生效"));
            ui.add_space(t.space_2);
            // 主动作放在最上面：表单较长，按钮不能藏到滚动区下面。
            ui.horizontal(|ui| {
                if widgets::action_button(ui, &t, Icon::CheckCircle, "保存配置", ButtonStyle::Primary, cfg_exists).clicked() {
                    do_save = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Bolt, "重新载入", ButtonStyle::Quiet, true).clicked() {
                    do_reload = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Gear, "原文编辑", ButtonStyle::Quiet, cfg_exists).clicked() {
                    do_open = true;
                }
            });
            ui.add_space(t.space_3);
            widgets::field(ui, &t, "采集模式", &mut form.mode, "ocr / android / stream", 200.0);
            widgets::field(ui, &t, "时间过滤", &mut form.time_filter, "today / any", 200.0);
            widgets::field(ui, &t, "会话白名单", &mut form.chat_names, "多个用逗号分隔，留空不限", 320.0);
            widgets::field(ui, &t, "公司过滤", &mut form.companies, "多个用逗号分隔，留空不限", 320.0);
            widgets::field(ui, &t, "网关端口", &mut form.gateway_port, "8765", 140.0);
            ui.add_space(t.space_1);
            widgets::switch(ui, &t, "解析手机画面", &mut form.android_parse, "开启后手机上报的节点树才会转成消息转发");
            widgets::switch(ui, &t, "附件转发", &mut form.attach_enabled, "卡片里的 Excel 自动下载并发到微信");
            widgets::switch(ui, &t, "演练模式", &mut form.dry_run, "开启只打印不真发（联调用）；正式转发务必关闭");
            widgets::field(ui, &t, "微信驱动", &mut form.driver, "auto / ocr / wxauto", 200.0);
            widgets::field(ui, &t, "发送间隔(秒)", &mut form.interval, "5.0", 140.0);
            widgets::field(ui, &t, "积压上限(分)", &mut form.max_backlog_min, "30", 140.0);
            widgets::field(ui, &t, "数据保留(天)", &mut form.keep_days, "90", 140.0);
            ui.add_space(t.space_2);
            widgets::notice(
                ui, &t, NoticeKind::Warning,
                "保存后必须重启引擎",
                "配置由 Python 转发引擎在启动时读取。请按 F6 停止、再按 F5 开始；保存时会自动生成 config.yaml.bak 备份。",
            );
            if let Some(e) = &cfg_err {
                ui.add_space(t.space_2);
                widgets::notice(ui, &t, NoticeKind::Danger, "配置文件不可用", e);
            }
        });

        ui.add_space(t.space_3);

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Grid, "路由规则", Some(&format!("{} 条 · 自上而下匹配", routes.len())));
            ui.add_space(t.space_2);
            if routes.is_empty() {
                widgets::empty_state(ui, &t, "尚未配置路由规则", "在设置中心的原文编辑里添加 routes: 段落");
            }
            for (i, r) in routes.iter().enumerate() {
                egui::Frame::default()
                    .fill(if i % 2 == 1 { t.bg_subtle } else { t.bg_surface_2 })
                    .corner_radius(t.r_sm)
                    .inner_margin(t.space_3)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.colored_label(t.text_strong, RichText::new(format!("{}. {}", i + 1, dash(&r.name))).size(13.0).strong());
                        ui.add_space(t.space_1);
                        ui.horizontal(|ui| {
                            ui.colored_label(t.text_faint, RichText::new("目标").size(11.0));
                            ui.colored_label(t.text, RichText::new(dash(&r.targets)).size(11.0));
                            ui.add_space(t.space_4);
                            ui.colored_label(t.text_faint, RichText::new("公司").size(11.0));
                            ui.colored_label(t.text, RichText::new(dash(&r.companies)).size(11.0));
                        });
                        ui.horizontal(|ui| {
                            ui.colored_label(t.text_faint, RichText::new("关键字").size(11.0));
                            ui.colored_label(t.text, RichText::new(dash(&r.keywords)).size(11.0));
                            ui.add_space(t.space_4);
                            ui.colored_label(t.text_faint, RichText::new("模板").size(11.0));
                            ui.colored_label(t.text_muted, RichText::new(data::shorten(&dash(&r.template), 46)).size(11.0));
                        });
                    });
                ui.add_space(t.space_2);
            }
        });

        self.rule_form = form;
        if do_reload {
            self.config.reload();
            self.rule_form = RuleForm::from_doc(&self.config);
            self.settings_text = self.config.text.clone();
            self.settings_dirty = false;
            self.push_log(self.t.success, "OK", "已重新载入 config.yaml（未保存的修改已丢弃）");
        }
        if do_save {
            let n = self.rule_form.apply(&mut self.config);
            match self.config.save() {
                Ok(msg) => {
                    self.settings_text = self.config.text.clone();
                    self.settings_dirty = false;
                    self.push_log(self.t.success, "OK", format!("规则已写入 {n} 个键 · {msg}"));
                    self.push_log(self.t.warning, "WARN", "配置已保存，请按 F6 停止、F5 重启转发引擎后生效");
                }
                Err(e) => self.push_log(self.t.danger, "ERR", format!("保存配置失败：{e}")),
            }
        }
        if do_open {
            if datasource::open_path(&datasource::config_path()) {
                self.push_log(self.t.success, "OK", "已用系统编辑器打开 config.yaml");
            } else {
                self.push_log(self.t.warning, "WARN", "打开配置失败，请用「设置中心 → 原文编辑」");
            }
        }
    }

    // ────────────────────────── 3. 漏转对账 ──────────────────────────

    pub(crate) fn page_reconcile(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let rows: Vec<ReconcileRow> = self.recon_rows.clone();
        let err = self.recon_error.clone();
        let official = self.snapshot.official_exe.clone();
        let kpi = self.snapshot.kpi.clone();
        let mut do_refresh = false;
        let mut do_export = false;
        let mut do_open = false;
        let mut do_check = false;

        widgets::notice(
            ui, &t, NoticeKind::Info,
            "这一页查的是「桥内漏投」，不是钉钉库跨库对账",
            "清单来自 bridge.db：所有从未出现过成功投递记录的消息。真正的「钉钉电脑端本地库 vs 本机入库」比对由 Python 引擎的 reconcile 模块完成，需要钉钉客户端 + pycryptodome，日志里会写结论。",
        );
        ui.add_space(t.space_3);

        let pending = rows.iter().filter(|r| r.attempts == 0).count();
        let fail_only = rows.iter().filter(|r| r.attempts > 0 && r.last_status == "fail").count();
        ui.columns(4, |cols| {
            let cards: [(Color32, &str, String, Icon, &str); 4] = [
                (t.danger, "疑似漏投", rows.len().to_string(), Icon::CheckCircle, "从未成功投递"),
                (t.warning, "一次未投", pending.to_string(), Icon::Clock, "入库后无投递尝试"),
                (t.danger, "仅失败", fail_only.to_string(), Icon::Bolt, "有尝试但全部失败"),
                (t.success, "累计成功率", format!("{:.1}%", kpi.success_rate), Icon::Chart, "含重试记录"),
            ];
            for (i, (accent, label, value, ic, foot)) in cards.iter().enumerate() {
                widgets::kpi_card(&mut cols[i], &t, label, value.as_str(), *accent, *ic, foot);
            }
        });
        ui.add_space(t.space_3);

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Database, "疑似漏投清单", Some(&format!("最近 {RECON_LIMIT} 条 · 只读")));
            ui.add_space(t.space_2);
            ui.horizontal(|ui| {
                if widgets::action_button(ui, &t, Icon::Bolt, "刷新清单", ButtonStyle::Quiet, true).clicked() {
                    do_refresh = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Database, "导出 CSV", ButtonStyle::Quiet, !rows.is_empty()).clicked() {
                    do_export = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::CheckCircle, "启动自检", ButtonStyle::Quiet, true).clicked() {
                    do_check = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Gear, "正式版对账", ButtonStyle::Quiet, official.is_some()).clicked() {
                    do_open = true;
                }
            });
            ui.add_space(t.space_3);
            if let Some(e) = &err {
                widgets::notice(ui, &t, NoticeKind::Danger, "查询失败", e);
                ui.add_space(t.space_2);
            }
            widgets::table_header(
                ui, &t,
                &["时间", "发送者", "分类", "尝试", "目标", "摘要 / 最后错误"],
                &[0.02, 0.20, 0.32, 0.42, 0.49, 0.66],
            );
            egui::ScrollArea::vertical()
                .id_salt("recon_table_scroll")
                .max_height(300.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    if rows.is_empty() {
                        widgets::empty_state(ui, &t, "没有疑似漏投", "所有入库消息都至少成功投递过一次");
                    }
                    for (i, r) in rows.iter().enumerate() {
                        let (rect, _) = widgets::table_row(ui, &t, i, 28.0, false);
                        widgets::cell(ui, rect, 0.02, &r.when, t.text_muted, true);
                        widgets::cell(ui, rect, 0.20, &r.sender, t.text, false);
                        widgets::cell(ui, rect, 0.32, r.category(), t.warning, false);
                        widgets::cell(ui, rect, 0.42, &r.attempts.to_string(), t.text_muted, true);
                        widgets::cell(ui, rect, 0.49, if r.target.is_empty() { "—" } else { &r.target }, t.text_muted, false);
                        let detail = if r.last_error.is_empty() { r.summary.clone() } else { format!("{} · {}", r.summary, data::shorten(&r.last_error, 22)) };
                        widgets::cell(ui, rect, 0.66, &detail, t.text_muted, false);
                    }
                });
        });
        ui.add_space(t.space_3);
        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Info, "排查建议", None);
            ui.add_space(t.space_2);
            for line in [
                "「一次未投」通常是入库后被过滤规则拦下，或引擎在投递前被停止 —— 先确认 routes / chat_names / dry_run。",
                "「仅失败」看最后错误：找不到会话、微信窗口被最小化、OCR 匹配阈值过高，都会导致投递失败。",
                "钉钉本地库对账的结论在实时日志里以 WARNING 输出；要详细报告请用 Python 正式版。",
            ] {
                ui.horizontal(|ui| {
                    ui.colored_label(t.primary, RichText::new("•").size(13.0));
                    ui.colored_label(t.text_muted, RichText::new(line).size(11.0));
                });
            }
        });

        if do_export {
            self.export("漏投清单.csv", &data::reconcile_csv(&rows));
        }
        if do_refresh {
            self.reload_reconcile();
            self.push_log(self.t.success, "OK", "漏投清单已重新查询（只读）");
        }
        if do_check {
            self.start(runner::Mode::Check);
        }
        if do_open {
            if let Some(exe) = official {
                if datasource::open_official(&exe) {
                    self.push_log(self.t.success, "OK", "已打开正式版窗口（完整对账与设置在那里）");
                } else {
                    self.push_log(self.t.warning, "WARN", "打开正式版失败");
                }
            }
        }
    }

    // ────────────────────────── 4. 设置中心 ──────────────────────────

    pub(crate) fn page_settings(&mut self, ui: &mut egui::Ui) {
        let t = self.t;
        let mut text = self.settings_text.clone();
        let mut keep = self.keep_days.clone();
        let plan = self.cleanup_plan.clone();
        let armed = self.cleanup_armed;
        let result = self.cleanup_result.clone();
        let db_path = self.snapshot.db_path.clone();
        let own_db = datasource::own_db();
        let engine_running = self.worker.is_some();
        let can_clean = own_db.is_some() && !engine_running;
        let backend_short = match &self.backend {
            Ok(b) => b.short(),
            Err(_) => "未找到引擎".to_owned(),
        };
        let mut do_save = false;
        let mut do_reload = false;
        let mut do_open_dir = false;
        let mut do_open_cfg = false;
        let mut do_open_log = false;
        let mut do_preview = false;
        let mut do_arm = false;
        let mut do_clean = false;

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Info, "运行环境", Some("本页路径均可直接打开"));
            ui.add_space(t.space_2);
            ui.columns(2, |cols| {
                let left: Vec<(&str, String)> = vec![
                    ("版本", format!("Rust 独立版 v{}", env!("CARGO_PKG_VERSION"))),
                    ("转发引擎", backend_short),
                    ("安装目录", datasource::install_dir().display().to_string()),
                    ("配置文件", datasource::config_path().display().to_string()),
                ];
                let right: Vec<(&str, String)> = vec![
                    ("数据源", db_path.as_deref().map(datasource::display_path).unwrap_or_else(|| "未找到 bridge.db".to_owned())),
                    ("写入权限", if own_db.is_some() { "自带库 · 可维护".to_owned() } else { "正式版库 · 只读".to_owned() }),
                    ("网关端口", runner::GATEWAY_PORT.to_string()),
                    ("运行日志", startup::log_file("startup.log").display().to_string()),
                ];
                for (label, value) in &left {
                    widgets::kv(&mut cols[0], &t, label, value, t.text);
                }
                for (label, value) in &right {
                    widgets::kv(&mut cols[1], &t, label, value, t.text);
                }
            });
            ui.add_space(t.space_3);
            ui.horizontal(|ui| {
                if widgets::action_button(ui, &t, Icon::Grid, "安装目录", ButtonStyle::Quiet, true).clicked() {
                    do_open_dir = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Gear, "打开配置", ButtonStyle::Quiet, true).clicked() {
                    do_open_cfg = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Bolt, "运行日志", ButtonStyle::Quiet, true).clicked() {
                    do_open_log = true;
                }
            });
        });

        ui.add_space(t.space_3);

        // 维护区排在原文编辑之前：清理是日常操作，不能藏在长文本框下面。
        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Database, "消息数据库维护", Some("清理旧数据 + VACUUM"));
            ui.add_space(t.space_2);
            widgets::field(ui, &t, "保留天数", &mut keep, "90（0 = 永久保留，只清孤儿记录）", 200.0);
            ui.add_space(t.space_2);
            ui.horizontal(|ui| {
                if widgets::action_button(ui, &t, Icon::Chart, "预览清理", ButtonStyle::Quiet, own_db.is_some()).clicked() {
                    do_preview = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Database, "执行清理", ButtonStyle::Danger, can_clean && !armed).clicked() {
                    do_arm = true;
                }
                if armed {
                    ui.add_space(t.space_2);
                    if widgets::action_button(ui, &t, Icon::CheckCircle, "确认删除", ButtonStyle::Danger, true).clicked() {
                        do_clean = true;
                    }
                }
            });
            ui.add_space(t.space_2);
            if !can_clean {
                let why = if own_db.is_none() {
                    "当前接的是 Python 正式版数据库，Rust 版永远只读，清理请使用正式版设置中心。"
                } else {
                    "转发引擎正在运行，VACUUM 需要独占数据库。请先按 F6 停止再清理。"
                };
                widgets::notice(ui, &t, NoticeKind::Warning, "清理已锁定", why);
            } else if armed {
                widgets::notice(ui, &t, NoticeKind::Danger, "请二次确认", "删除不可撤销（配置备份只覆盖 config.yaml，不含数据库）。确认后才会真正删行并压缩数据库。");
            }
            if let Some(p) = &plan {
                ui.add_space(t.space_2);
                if !p.error.is_empty() {
                    widgets::notice(ui, &t, NoticeKind::Danger, "体检失败", &p.error);
                } else {
                    for line in p.summary() {
                        ui.horizontal(|ui| {
                            ui.colored_label(t.primary, RichText::new("•").size(12.0));
                            ui.colored_label(t.text_muted, RichText::new(line).size(11.0));
                        });
                    }
                    if p.nothing_to_do() {
                        ui.add_space(t.space_1);
                        ui.colored_label(t.success, RichText::new("没有需要清理的数据").size(11.0));
                    }
                }
            }
            if let Some(r) = &result {
                ui.add_space(t.space_2);
                widgets::notice(ui, &t, NoticeKind::Success, "清理结果", r);
            }
            ui.add_space(t.space_2);
            widgets::notice(
                ui, &t, NoticeKind::Info,
                "清理边界",
                "只删超龄的 messages / deliveries 行与孤儿投递记录，附件文件（data/attachments）一律不动。口径与 Python 正式版 bridge/dbclean.py 一致。",
            );
        });

        ui.add_space(t.space_3);

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Sliders, "配置原文", Some("config.yaml · 保留注释，保存前自动备份"));
            ui.add_space(t.space_2);
            ui.horizontal(|ui| {
                if widgets::action_button(ui, &t, Icon::CheckCircle, "保存原文", ButtonStyle::Primary, true).clicked() {
                    do_save = true;
                }
                ui.add_space(t.space_2);
                if widgets::action_button(ui, &t, Icon::Bolt, "重新载入", ButtonStyle::Quiet, true).clicked() {
                    do_reload = true;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.colored_label(t.text_faint, RichText::new("改坏配置会导致引擎启动失败，.bak 可还原").size(11.0));
                });
            });
            ui.add_space(t.space_2);
            egui::ScrollArea::vertical()
                .id_salt("config_editor")
                .max_height(190.0)
                .auto_shrink([false; 2])
                .show(ui, |ui| {
                    ui.add_sized(
                        egui::Vec2::new(ui.available_width(), 170.0),
                        egui::TextEdit::multiline(&mut text)
                            .font(egui::FontId::monospace(11.0))
                            .desired_rows(8)
                            .code_editor(),
                    );
                });
        });

        ui.add_space(t.space_3);

        widgets::card(ui, &t, |ui| {
            widgets::section_header(ui, &t, Icon::Info, "关于与快捷键", None);
            ui.add_space(t.space_2);
            for (k, v) in [
                ("F5", "开始真实转发（拉起 Python 生产引擎）"),
                ("F6", "优雅停止（哨兵文件 → 收尾 → 退出）"),
                ("Ctrl+R", "刷新只读数据快照"),
                ("架构", "Rust / egui 界面 + Python 转发引擎，界面只读 bridge.db"),
                ("边界", "规则配置与跨库漏转对账仍由 Python 正式版承担更完整的实现"),
            ] {
                ui.horizontal(|ui| {
                    widgets::kbd(ui, &t, k);
                    ui.colored_label(t.text_muted, RichText::new(v).size(11.0));
                });
            }
        });

        self.settings_text = text.clone();
        self.settings_dirty = text != self.config.text;
        self.keep_days = keep;

        if do_open_dir {
            let _ = datasource::open_path(&datasource::install_dir());
        }
        if do_open_cfg {
            let _ = datasource::open_path(&datasource::config_path());
        }
        if do_open_log {
            let _ = datasource::open_path(&startup::log_file("startup.log"));
        }
        if do_reload {
            self.config.reload();
            self.settings_text = self.config.text.clone();
            self.settings_dirty = false;
            self.rule_form = RuleForm::from_doc(&self.config);
            self.push_log(self.t.success, "OK", "配置原文已重新载入");
        }
        if do_save {
            self.config.text = self.settings_text.clone();
            match self.config.save() {
                Ok(msg) => {
                    self.settings_dirty = false;
                    self.rule_form = RuleForm::from_doc(&self.config);
                    self.push_log(self.t.success, "OK", msg);
                    self.push_log(self.t.warning, "WARN", "配置已保存，请按 F6 停止、F5 重启转发引擎后生效");
                }
                Err(e) => self.push_log(self.t.danger, "ERR", format!("保存失败：{e}")),
            }
        }
        if do_preview {
            self.preview_cleanup();
        }
        if do_arm {
            if self.cleanup_plan.is_none() {
                self.preview_cleanup();
            }
            self.cleanup_armed = self.cleanup_plan.as_ref().map(|p| p.error.is_empty() && !p.nothing_to_do()).unwrap_or(false);
            if self.cleanup_armed {
                self.push_log(self.t.warning, "WARN", "清理待二次确认：请核对上方预览清单，再点「确认删除」");
            }
        }
        if do_clean {
            self.run_cleanup();
        }
    }

    fn preview_cleanup(&mut self) {
        let Some(db) = datasource::own_db() else {
            self.cleanup_result = Some("当前数据源不是独立版自带库，拒绝写入".to_owned());
            return;
        };
        let keep = match self.keep_value() {
            Ok(v) => v,
            Err(e) => {
                self.cleanup_plan = None;
                self.cleanup_result = Some(e);
                return;
            }
        };
        match dbclean::scan(&db, keep) {
            Ok(p) => {
                let line = if p.error.is_empty() {
                    format!("体检完成 · 保留 {keep} 天 · 预计删 {} 条消息 / {} 条投递记录", p.message_old, p.doomed())
                } else {
                    p.error.clone()
                };
                self.cleanup_plan = Some(p);
                self.cleanup_result = None;
                self.push_log(self.t.success, "OK", line);
            }
            Err(e) => {
                self.cleanup_plan = None;
                self.cleanup_result = Some(e.clone());
                self.push_log(self.t.danger, "ERR", e);
            }
        }
    }

    fn run_cleanup(&mut self) {
        let Some(db) = datasource::own_db() else {
            self.push_log(self.t.danger, "ERR", "拒绝清理：当前数据库不是独立版自带库");
            return;
        };
        if self.worker.is_some() {
            self.push_log(self.t.warning, "WARN", "请先按 F6 停止转发，再执行清理");
            return;
        }
        let keep = match self.keep_value() {
            Ok(v) => v,
            Err(e) => {
                self.push_log(self.t.danger, "ERR", e);
                return;
            }
        };
        let plan = match dbclean::scan(&db, keep) {
            Ok(p) if p.error.is_empty() => p,
            Ok(p) => {
                self.push_log(self.t.danger, "ERR", p.error);
                return;
            }
            Err(e) => {
                self.push_log(self.t.danger, "ERR", e);
                return;
            }
        };
        match dbclean::execute(&db, &plan, true) {
            Ok(r) => {
                if !r.error.is_empty() {
                    self.cleanup_result = Some(r.error.clone());
                    self.push_log(self.t.danger, "ERR", r.error);
                } else {
                    let msg = format!(
                        "已删 {} 条消息 / {} 条投递记录 · VACUUM {} · 释放 {}",
                        r.deleted_messages,
                        r.deleted_deliveries,
                        if r.vacuumed { "成功" } else { "未执行" },
                        dbclean::human_size(r.freed())
                    );
                    self.cleanup_result = Some(msg.clone());
                    self.push_log(self.t.success, "OK", msg);
                    if !r.note.is_empty() {
                        self.push_log(self.t.warning, "WARN", r.note);
                    }
                    self.refresh(false);
                    self.refresh_pages();
                }
            }
            Err(e) => {
                self.cleanup_result = Some(e.clone());
                self.push_log(self.t.danger, "ERR", format!("清理失败：{e}"));
            }
        }
        self.cleanup_plan = None;
        self.cleanup_armed = false;
    }
}

fn dash(s: &str) -> String {
    if s.trim().is_empty() {
        "—".to_owned()
    } else {
        s.to_owned()
    }
}
